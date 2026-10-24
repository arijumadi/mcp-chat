pub mod approvals;
pub mod context;
pub mod permissions;
pub mod prompt;
pub mod runner;
pub mod tools;
pub mod types;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::db::repo::{conversations, messages, providers, settings};
use crate::events::CoreEvent;
use crate::mcp::McpManager;
use crate::models::{Conversation, Message, PartPayload, Provider};
use crate::secrets::SecretStore;
use crate::{CoreError, Result};

use self::approvals::{ApprovalAnswer, ApprovalRegistry, InputAnswer, InputRegistry};
use self::tools::ToolExecutor;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertedMessage {
    pub text: String,
    pub attachments: Vec<crate::attachments::Attachment>,
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
        let (events, _) = broadcast::channel(2048);
        let approvals = Arc::new(ApprovalRegistry::default());
        let inputs = Arc::new(InputRegistry::default());
        let executor = Arc::new(ToolExecutor {
            db: db.clone(),
            mcp: mcp.clone(),
            approvals: approvals.clone(),
            inputs: inputs.clone(),
            events: events.clone(),
            output_dir: output_dir.clone(),
            limiter: Arc::new(Semaphore::new(4)),
        });
        Arc::new(Self {
            db,
            secrets,
            mcp,
            events,
            approvals,
            inputs,
            executor,
            output_dir,
            attachments_dir,
            turns: Mutex::new(HashMap::new()),
            auto_approve: Mutex::new(HashSet::new()),
        })
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

    fn start_turn(self: &Arc<Self>, conversation_id: &str) -> Result<()> {
        let token = CancellationToken::new();
        {
            let mut turns = self.turns.lock().expect("turns lock");
            if turns.contains_key(conversation_id) {
                return Err(CoreError::Message(
                    "This conversation is already generating a response.".into(),
                ));
            }
            turns.insert(conversation_id.to_owned(), token.clone());
        }
        let engine = self.clone();
        let id = conversation_id.to_owned();
        tokio::spawn(async move {
            if let Err(error) = runner::run(engine.clone(), &id, token).await {
                tracing::debug!(conversation = %id, error = %error.user_message(), "turn ended with error");
            }
            engine.turns.lock().expect("turns lock").remove(&id);
        });
        Ok(())
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
