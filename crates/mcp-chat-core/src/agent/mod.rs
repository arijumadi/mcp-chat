pub mod approvals;
pub mod capabilities;
pub mod context;
pub mod diff;
pub mod edit;
pub mod file_locks;
pub mod patch;
pub mod permissions;
pub mod prompt;
pub mod runner;
pub mod shell_guard;
pub mod tools;
pub mod types;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::{Semaphore, broadcast, oneshot};
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::db::repo::{conversation_servers, conversations, messages, providers, settings};
use crate::events::CoreEvent;
use crate::mcp::McpManager;
use crate::models::{Conversation, Message, PartPayload, Provider};
use crate::secrets::SecretStore;
use crate::{CoreError, Result};

use self::approvals::{ApprovalAnswer, ApprovalRegistry, InputAnswer, InputRegistry};
use self::tools::{SubagentHost, ToolExecutor};
use futures::future::BoxFuture;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertedMessage {
    pub text: String,
    pub attachments: Vec<crate::attachments::Attachment>,
}

/// Holds a conversation's turn slot while alive. See [`ChatEngine::claim_turn`].
pub(crate) struct TurnSlot {
    engine: Arc<ChatEngine>,
    id: String,
}

impl Drop for TurnSlot {
    fn drop(&mut self) {
        self.engine
            .turns
            .lock()
            .expect("turns lock")
            .remove(&self.id);
    }
}

pub struct ChatEngine {
    pub db: Db,
    pub secrets: Arc<dyn SecretStore>,
    pub mcp: Arc<McpManager>,
    pub events: broadcast::Sender<CoreEvent>,
    pub approvals: Arc<ApprovalRegistry>,
    pub inputs: Arc<InputRegistry>,
    pub executor: Arc<ToolExecutor>,
    pub output_dir: PathBuf,
    pub attachments_dir: PathBuf,
    turns: Mutex<HashMap<String, CancellationToken>>,
    auto_approve: Mutex<HashSet<String>>,
}

impl ChatEngine {
    pub fn new(
        db: Db,
        secrets: Arc<dyn SecretStore>,
        mcp: Arc<McpManager>,
        output_dir: PathBuf,
        attachments_dir: PathBuf,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(8192);
        let approvals = Arc::new(ApprovalRegistry::default());
        let inputs = Arc::new(InputRegistry::default());
        let executor = Arc::new(ToolExecutor {
            db: db.clone(),
            mcp: mcp.clone(),
            approvals: approvals.clone(),
            inputs: inputs.clone(),
            events: events.clone(),
            output_dir: output_dir.clone(),
            attachments_dir: attachments_dir.clone(),
            limiter: Arc::new(Semaphore::new(4)),
            file_locks: crate::agent::file_locks::FileLocks::default(),
            subagent_host: std::sync::OnceLock::new(),
        });
        let engine = Arc::new(Self {
            db,
            secrets,
            mcp,
            events,
            approvals,
            inputs,
            executor: executor.clone(),
            output_dir,
            attachments_dir,
            turns: Mutex::new(HashMap::new()),
            auto_approve: Mutex::new(HashSet::new()),
        });
        let host: Arc<dyn SubagentHost> = Arc::new(SubagentHostRef {
            engine: Arc::downgrade(&engine),
        });
        let _ = executor.subagent_host.set(host);
        engine
    }

    /// Launch a focused child conversation. The child gets fresh context, the
    /// same workspace/model/tool selection, and its own message history. It is
    /// hidden from the main conversation list and reports back asynchronously.
    pub async fn launch_subagent(
        self: &Arc<Self>,
        parent_id: &str,
        description: &str,
        prompt: &str,
        session_id: Option<&str>,
        part_id: &str,
    ) -> Result<(String, String)> {
        let parent = conversations::get(&self.db.pool, parent_id).await?;

        if let Some(session_id) = session_id {
            let child = conversations::get(&self.db.pool, session_id).await?;
            if child.parent_id.as_deref() != Some(parent_id) {
                return Err(CoreError::Message(
                    "That conversation is not a child of this chat.".into(),
                ));
            }
            self.mark_subagent_part(parent_id, part_id, &child.id).await;
            self.append_subagent_prompt(&child.id, prompt, false)
                .await?;
            let running = self.in_flight(&child.id);
            if !running {
                self.inherit_subagent_auto(&parent, &child.id);
                let done = self.start_turn_with_done(&child.id)?;
                self.spawn_subagent_supervisor(
                    parent.id.clone(),
                    child.id.clone(),
                    child.title.clone(),
                    done,
                );
            }
            let content = if running {
                format!(
                    "Follow-up sent to the running subagent (sessionID: {}). It will report again when this round finishes.",
                    child.id
                )
            } else {
                format!(
                    "Follow-up sent. The subagent is running in the background (sessionID: {}).",
                    child.id
                )
            };
            return Ok((child.id, content));
        }

        if parent.parent_id.is_some() {
            return Err(CoreError::Message(
                "Subagent depth limit reached. A subagent cannot start another subagent.".into(),
            ));
        }

        let description = description.trim();
        let description = if description.is_empty() {
            "Subagent"
        } else {
            description
        };
        let title: String = description.chars().take(120).collect();
        let child = conversations::create(
            &self.db.pool,
            &title,
            parent.provider_id.as_deref(),
            parent.model.as_deref(),
        )
        .await?;
        conversations::set_parent(&self.db.pool, &child.id, &parent.id).await?;
        conversations::set_workspace(&self.db.pool, &child.id, parent.workspace_root.as_deref())
            .await?;
        conversations::set_reasoning_effort(&self.db.pool, &child.id, &parent.reasoning_effort)
            .await?;
        conversations::set_tool_mode(&self.db.pool, &child.id, &parent.tool_mode).await?;
        conversations::set_system_prompt(&self.db.pool, &child.id, parent.system_prompt.as_deref())
            .await?;
        conversations::set_max_steps(
            &self.db.pool,
            &child.id,
            if parent.max_steps > 0 {
                parent.max_steps
            } else {
                30
            },
        )
        .await?;
        if parent.servers_configured {
            let server_ids = conversation_servers::selected(&self.db.pool, &parent.id).await?;
            conversation_servers::set(&self.db.pool, &child.id, &server_ids).await?;
        }
        self.mark_subagent_part(parent_id, part_id, &child.id).await;
        self.append_subagent_prompt(&child.id, prompt, true).await?;
        self.inherit_subagent_auto(&parent, &child.id);
        let done = self.start_turn_with_done(&child.id)?;
        self.spawn_subagent_supervisor(
            parent.id.clone(),
            child.id.clone(),
            child.title.clone(),
            done,
        );
        let content = format!(
            "The subagent is running in the background (sessionID: {}). You will receive a <subagent> report when it goes idle. Do not poll or duplicate its work; work on non-overlapping tasks or briefly tell the user and end your response.",
            child.id
        );
        Ok((child.id, content))
    }

    async fn mark_subagent_part(&self, parent_id: &str, part_id: &str, child_id: &str) {
        let Ok(part) = messages::get_part(&self.db.pool, part_id).await else {
            return;
        };
        let mut payload = part.payload();
        payload["subagentSessionId"] = serde_json::json!(child_id);
        payload["subagentState"] = serde_json::json!("running");
        if messages::update_part_payload(&self.db.pool, part_id, &payload)
            .await
            .is_ok()
        {
            let _ = self.events.send(CoreEvent::Part {
                conversation_id: parent_id.to_owned(),
                message_id: part.message_id,
                part_id: part_id.to_owned(),
                kind: "tool_call".into(),
                payload,
            });
        }
    }

    async fn append_subagent_prompt(
        &self,
        child_id: &str,
        prompt: &str,
        first: bool,
    ) -> Result<()> {
        let message = messages::create(&self.db.pool, child_id, "user", "completed").await?;
        let text = if first {
            format!(
                "You are a subagent spawned by another Jarvis conversation. Work on the task below and return a concise, complete report for the parent agent. Do not ask the user unless truly blocked.\n\n{prompt}"
            )
        } else {
            prompt.to_owned()
        };
        messages::append_part(
            &self.db.pool,
            &message.id,
            "text",
            &PartPayload::Text { text },
        )
        .await?;
        messages::append_part(
            &self.db.pool,
            &message.id,
            "time",
            &PartPayload::Text {
                text: crate::util::message_time_note(message.created_at),
            },
        )
        .await?;
        conversations::touch(&self.db.pool, child_id).await?;
        Ok(())
    }

    fn inherit_subagent_auto(&self, parent: &Conversation, child_id: &str) {
        if self.auto_approve(&parent.id) {
            self.set_auto_approve(child_id, true);
        }
    }

    fn spawn_subagent_supervisor(
        self: &Arc<Self>,
        parent_id: String,
        child_id: String,
        description: String,
        done: oneshot::Receiver<()>,
    ) {
        let engine = self.clone();
        tokio::spawn(async move {
            engine
                .supervise_subagent(parent_id, child_id, description, done)
                .await;
        });
    }

    async fn supervise_subagent(
        self: Arc<Self>,
        parent_id: String,
        child_id: String,
        description: String,
        mut done: oneshot::Receiver<()>,
    ) {
        loop {
            let _ = (&mut done).await;
            let history = messages::list_for_conversation(&self.db.pool, &child_id)
                .await
                .unwrap_or_default();
            let has_pending_user = history
                .last()
                .is_some_and(|item| item.message.role == "user");
            if has_pending_user {
                match self.start_turn_with_done(&child_id) {
                    Ok(next) => {
                        done = next;
                        continue;
                    }
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                }
            }
            let (state, report) = subagent_report(&history);
            self.finish_subagent(&parent_id, &child_id, &description, state, &report)
                .await;
            self.set_auto_approve(&child_id, false);
            break;
        }
    }

    async fn finish_subagent(
        self: &Arc<Self>,
        parent_id: &str,
        child_id: &str,
        description: &str,
        state: &str,
        report: &str,
    ) {
        let preview = crate::util::truncate_chars(report, 4_000).0;
        if let Ok(history) = messages::list_for_conversation(&self.db.pool, parent_id).await {
            for message in &history {
                for part in &message.parts {
                    if part.kind != "tool_call" {
                        continue;
                    }
                    let mut payload = part.payload();
                    if payload
                        .get("subagentSessionId")
                        .and_then(serde_json::Value::as_str)
                        != Some(child_id)
                    {
                        continue;
                    }
                    if payload
                        .get("subagentState")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| value == "completed" || value == "failed")
                    {
                        continue;
                    }
                    payload["subagentState"] = serde_json::json!(state);
                    payload["resultPreview"] = serde_json::json!(preview);
                    payload["endedAt"] = serde_json::json!(crate::now_ms());
                    if state == "failed" {
                        payload["error"] = serde_json::json!(report);
                    }
                    let _ = messages::update_part_payload(&self.db.pool, &part.id, &payload).await;
                    let _ = self.events.send(CoreEvent::Part {
                        conversation_id: parent_id.to_owned(),
                        message_id: message.message.id.clone(),
                        part_id: part.id.clone(),
                        kind: "tool_call".into(),
                        payload,
                    });
                }
            }
        }

        let report = format!(
            "<subagent state=\"{state}\" description=\"{}\" sessionID=\"{child_id}\">\n{}\n</subagent>",
            description.replace('"', "'"),
            crate::util::truncate_chars(report, 12_000).0
        );
        let Ok(message) = messages::create(&self.db.pool, parent_id, "user", "synthetic").await
        else {
            return;
        };
        if messages::append_part(
            &self.db.pool,
            &message.id,
            "text",
            &PartPayload::Text { text: report },
        )
        .await
        .is_err()
        {
            return;
        }
        let _ = messages::append_part(
            &self.db.pool,
            &message.id,
            "time",
            &PartPayload::Text {
                text: crate::util::message_time_note(message.created_at),
            },
        )
        .await;
        let _ = conversations::touch(&self.db.pool, parent_id).await;
        let _ = self.events.send(CoreEvent::ConversationUpdated {
            conversation_id: parent_id.to_owned(),
        });
        if !self.in_flight(parent_id) {
            let _ = self.start_turn(parent_id);
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.events.subscribe()
    }

    pub fn set_auto_approve(&self, conversation_id: &str, enabled: bool) {
        let mut states = self.auto_approve.lock().expect("auto approve lock");
        if enabled {
            states.insert(conversation_id.to_owned());
        } else {
            states.remove(conversation_id);
        }
    }

    pub fn auto_approve(&self, conversation_id: &str) -> bool {
        self.auto_approve
            .lock()
            .expect("auto approve lock")
            .contains(conversation_id)
    }

    pub fn in_flight(&self, conversation_id: &str) -> bool {
        self.turns
            .lock()
            .expect("turns lock")
            .contains_key(conversation_id)
    }

    pub async fn send_message(
        self: &Arc<Self>,
        conversation_id: &str,
        text: &str,
    ) -> Result<Message> {
        self.send_message_with_attachments(conversation_id, text, &[])
            .await
    }

    pub async fn send_message_with_attachments(
        self: &Arc<Self>,
        conversation_id: &str,
        text: &str,
        attachment_ids: &[String],
    ) -> Result<Message> {
        let text = text.trim();
        if text.is_empty() && attachment_ids.is_empty() {
            return Err(CoreError::invalid("message has no text or attachments"));
        }
        if text.chars().count() > 100_000 {
            return Err(CoreError::invalid("message is too long"));
        }
        if attachment_ids.len() > crate::attachments::MAX_ATTACHMENTS {
            return Err(CoreError::invalid(format!(
                "Attach at most {} files at once.",
                crate::attachments::MAX_ATTACHMENTS
            )));
        }
        if self.in_flight(conversation_id) {
            return Err(CoreError::Message(
                "This conversation is already generating a response.".into(),
            ));
        }
        let conversation = conversations::get(&self.db.pool, conversation_id).await?;
        let message = messages::create(&self.db.pool, conversation_id, "user", "completed").await?;
        if !text.is_empty() {
            messages::append_part(
                &self.db.pool,
                &message.id,
                "text",
                &PartPayload::Text {
                    text: text.to_owned(),
                },
            )
            .await?;
        }
        messages::append_part(
            &self.db.pool,
            &message.id,
            "time",
            &PartPayload::Text {
                text: crate::util::message_time_note(message.created_at),
            },
        )
        .await?;
        let attachments = match crate::attachments::attach_to_message(
            &self.attachments_dir,
            conversation_id,
            &message.id,
            attachment_ids,
        )
        .await
        {
            Ok(attachments) => attachments,
            Err(error) => {
                let _ = messages::delete_message(&self.db.pool, &message.id).await;
                return Err(error);
            }
        };
        for attachment in &attachments {
            messages::append_part(
                &self.db.pool,
                &message.id,
                "attachment",
                &PartPayload::Attachment {
                    id: attachment.id.clone(),
                    name: attachment.name.clone(),
                    mime: attachment.mime.clone(),
                    size: attachment.size,
                    file_kind: attachment.kind.clone(),
                    path: attachment.path.clone(),
                    text: attachment.text.clone(),
                    truncated: attachment.truncated,
                },
            )
            .await?;
        }
        if conversation.title == "New chat" || conversation.title.trim().is_empty() {
            let title_source = if text.is_empty() {
                attachments
                    .first()
                    .map(|attachment| attachment.name.as_str())
                    .unwrap_or("New chat")
            } else {
                text
            };
            conversations::update_title(
                &self.db.pool,
                conversation_id,
                &conversations::title_from_text(title_source),
            )
            .await?;
        }
        conversations::touch(&self.db.pool, conversation_id).await?;
        let _ = self.events.send(CoreEvent::ConversationUpdated {
            conversation_id: conversation_id.to_owned(),
        });
        self.start_turn(conversation_id)?;
        Ok(message)
    }

    /// Stop the current turn, wait for it to settle, then start a fresh turn
    /// with the user's new message. This is the "steer" path used while the
    /// agent is looping.
    pub async fn steer_message_with_attachments(
        self: &Arc<Self>,
        conversation_id: &str,
        text: &str,
        attachment_ids: &[String],
    ) -> Result<Message> {
        if !self.in_flight(conversation_id) {
            return self
                .send_message_with_attachments(conversation_id, text, attachment_ids)
                .await;
        }
        self.stop(conversation_id);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        while self.in_flight(conversation_id) {
            if tokio::time::Instant::now() >= deadline {
                return Err(CoreError::Message(
                    "The current turn is still stopping. Try steering again.".into(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        self.send_message_with_attachments(conversation_id, text, attachment_ids)
            .await
    }

    pub async fn regenerate(self: &Arc<Self>, conversation_id: &str) -> Result<()> {
        if self.in_flight(conversation_id) {
            return Err(CoreError::Message(
                "This conversation is already generating a response.".into(),
            ));
        }
        let history = messages::list_for_conversation(&self.db.pool, conversation_id).await?;
        let last_user = history
            .iter()
            .rev()
            .find(|message| message.message.role == "user" && message.message.status != "synthetic")
            .ok_or_else(|| CoreError::Message("There is no user message to regenerate.".into()))?;
        messages::delete_after(&self.db.pool, conversation_id, &last_user.message.id).await?;
        conversations::touch(&self.db.pool, conversation_id).await?;
        self.start_turn(conversation_id)?;
        Ok(())
    }

    /// Move a sent user message back into the composer. The target message and
    /// everything after it are removed, so the next send creates a clean new
    /// user turn. Returns the original text.
    /// Resume a turn that stopped at the max-step limit. Adds a hidden
    /// synthetic user message and starts a fresh step budget.
    pub async fn continue_turn(self: &Arc<Self>, conversation_id: &str) -> Result<()> {
        if self.in_flight(conversation_id) {
            return Err(CoreError::Message(
                "This conversation is already generating a response.".into(),
            ));
        }
        let message = messages::create(&self.db.pool, conversation_id, "user", "synthetic").await?;
        messages::append_part(
            &self.db.pool,
            &message.id,
            "text",
            &PartPayload::Text {
                text: crate::agent::types::CONTINUE_TURN_PROMPT.into(),
            },
        )
        .await?;
        conversations::touch(&self.db.pool, conversation_id).await?;
        let _ = self.events.send(CoreEvent::ConversationUpdated {
            conversation_id: conversation_id.to_owned(),
        });
        self.start_turn(conversation_id)?;
        Ok(())
    }

    pub async fn revert_message(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<RevertedMessage> {
        if self.in_flight(conversation_id) {
            return Err(CoreError::Message(
                "Stop the current turn before reverting a message.".into(),
            ));
        }
        let history = messages::list_for_conversation(&self.db.pool, conversation_id).await?;
        let index = history
            .iter()
            .position(|item| item.message.id == message_id)
            .ok_or_else(|| CoreError::NotFound("message".into()))?;
        let target = &history[index];
        if target.message.conversation_id != conversation_id {
            return Err(CoreError::invalid(
                "message does not belong to conversation",
            ));
        }
        if target.message.role != "user" || target.message.status == "synthetic" {
            return Err(CoreError::invalid("only user messages can be reverted"));
        }
        let text = crate::agent::context::text_parts(&target.parts);
        let attachment_paths: Vec<String> = target
            .parts
            .iter()
            .filter(|part| part.kind == "attachment")
            .filter_map(|part| {
                part.payload()
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|path| std::path::Path::new(path).is_file())
            .collect();
        if text.trim().is_empty() && attachment_paths.is_empty() {
            return Err(CoreError::invalid(
                "message has no text or attachments to revert",
            ));
        }

        for item in &history[index..] {
            for part in &item.parts {
                if part.kind != "tool_call" {
                    continue;
                }
                if let Some(call_id) = part
                    .payload()
                    .get("callId")
                    .and_then(serde_json::Value::as_str)
                {
                    let _ = crate::agent::tools::remove_spilled(&self.output_dir, call_id).await;
                }
            }
        }

        let restored = if attachment_paths.is_empty() {
            Vec::new()
        } else {
            crate::attachments::stage(&self.attachments_dir, &attachment_paths)
                .await
                .unwrap_or_default()
        };

        messages::delete_from(&self.db.pool, conversation_id, message_id).await?;
        for item in &history[index..] {
            crate::attachments::delete_message_dir(
                &self.attachments_dir,
                conversation_id,
                &item.message.id,
            )
            .await;
        }
        let remaining = messages::list_for_conversation(&self.db.pool, conversation_id).await?;
        if remaining.is_empty() {
            conversations::update_title(&self.db.pool, conversation_id, "New chat").await?;
        }
        conversations::touch(&self.db.pool, conversation_id).await?;
        let _ = self.events.send(CoreEvent::ConversationUpdated {
            conversation_id: conversation_id.to_owned(),
        });
        Ok(RevertedMessage {
            text,
            attachments: restored,
        })
    }

    /// Take a conversation's single turn slot, or fail with `busy` if a turn
    /// (or a manual compaction) already holds it. Dropping the returned slot
    /// releases it, including on early return and panic.
    pub(crate) fn claim_turn(
        self: &Arc<Self>,
        conversation_id: &str,
        busy: &str,
    ) -> Result<(TurnSlot, CancellationToken)> {
        let token = CancellationToken::new();
        let mut turns = self.turns.lock().expect("turns lock");
        if turns.contains_key(conversation_id) {
            return Err(CoreError::Message(busy.into()));
        }
        turns.insert(conversation_id.to_owned(), token.clone());
        Ok((
            TurnSlot {
                engine: self.clone(),
                id: conversation_id.to_owned(),
            },
            token,
        ))
    }

    fn start_turn(self: &Arc<Self>, conversation_id: &str) -> Result<()> {
        drop(self.start_turn_with_done(conversation_id)?);
        Ok(())
    }

    fn start_turn_with_done(
        self: &Arc<Self>,
        conversation_id: &str,
    ) -> Result<oneshot::Receiver<()>> {
        let (slot, token) = self.claim_turn(
            conversation_id,
            "This conversation is already generating a response.",
        )?;
        let engine = self.clone();
        let id = conversation_id.to_owned();
        let (done_tx, done_rx) = oneshot::channel();
        tokio::spawn(async move {
            // The slot is released when this task ends, however it ends.
            let _slot = slot;
            if let Err(error) = runner::run(engine.clone(), &id, token).await {
                tracing::debug!(conversation = %id, error = %error.user_message(), "turn ended with error");
            }
            let _ = done_tx.send(());
        });
        Ok(done_rx)
    }

    pub fn stop(&self, conversation_id: &str) -> bool {
        if let Some(token) = self.turns.lock().expect("turns lock").get(conversation_id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    pub fn respond_approval(&self, call_id: &str, answer: ApprovalAnswer) -> bool {
        self.approvals.respond(call_id, answer)
    }

    pub fn respond_input(&self, call_id: &str, answer: InputAnswer) -> bool {
        self.inputs.respond(call_id, answer)
    }

    pub async fn read_tool_output(&self, call_id: &str, offset: usize) -> Result<String> {
        tools::read_spilled(&self.output_dir, call_id, offset).await
    }

    pub async fn resolve(
        &self,
        conversation: &Conversation,
    ) -> Result<(Provider, String, Option<String>, i64, i64)> {
        let provider_id = match &conversation.provider_id {
            Some(id) => Some(id.clone()),
            None => settings::get(&self.db.pool, "last_provider_id").await?,
        };
        let provider = match provider_id {
            Some(id) => providers::get(&self.db.pool, &id).await?,
            None => providers::list(&self.db.pool)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CoreError::Message(
                        "Add a provider in Settings before sending a message.".into(),
                    )
                })?,
        };
        let model = conversation
            .model
            .clone()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| provider.model.clone());
        if model.trim().is_empty() {
            return Err(CoreError::Message(
                "Choose a model for this provider in Settings.".into(),
            ));
        }
        let account = provider
            .api_key_ref
            .clone()
            .unwrap_or_else(|| provider.id.clone());
        let api_key = self.secrets.get(&account).await?;
        let local = url::Url::parse(&provider.base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .map(|host| matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1"))
            .unwrap_or(false);
        if api_key.is_none() && provider.kind != "openai_compat" {
            return Err(CoreError::Message(
                "Add an API key for this provider in Settings.".into(),
            ));
        }
        if api_key.is_none() && provider.kind == "openai_compat" && !local {
            return Err(CoreError::Message(
                "Add an API key for this provider in Settings.".into(),
            ));
        }
        Ok((
            provider.clone(),
            model,
            api_key,
            provider.context_window.max(4_000),
            provider.max_output_tokens.max(256),
        ))
    }
}

impl SubagentHost for SubagentHostRef {
    fn run_subagent<'a>(
        &'a self,
        parent_id: &'a str,
        description: &'a str,
        prompt: &'a str,
        session_id: Option<&'a str>,
        part_id: &'a str,
    ) -> BoxFuture<'a, Result<(String, String)>> {
        Box::pin(async move {
            let engine = self.engine.upgrade().ok_or_else(|| {
                CoreError::Message("The subagent runner is not available.".into())
            })?;
            engine
                .launch_subagent(parent_id, description, prompt, session_id, part_id)
                .await
        })
    }
}

struct SubagentHostRef {
    engine: Weak<ChatEngine>,
}

fn subagent_report(history: &[crate::models::MessageWithParts]) -> (&'static str, String) {
    let Some(last) = history
        .iter()
        .rev()
        .find(|item| item.message.role == "assistant")
    else {
        return (
            "failed",
            "Subagent finished without a final answer.".to_owned(),
        );
    };
    if last.message.status == "error" {
        return (
            "failed",
            last.message
                .error
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "Subagent failed.".into()),
        );
    }
    let report = crate::agent::context::text_parts(&last.parts);
    if report.trim().is_empty() {
        (
            "failed",
            "Subagent finished without a final answer.".to_owned(),
        )
    } else {
        ("completed", report)
    }
}
