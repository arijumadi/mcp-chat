pub mod acp_turn;
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
use crate::db::repo::{
    acp_agents, conversation_servers, conversations, messages, providers, settings,
};
use crate::events::CoreEvent;
use crate::mcp::McpManager;
use crate::models::{Conversation, Message, MessageContext, PartPayload, Provider};
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
    pub contexts: Vec<MessageContext>,
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
    /// Live external ACP sessions, keyed by child conversation id.
    pub acp: Arc<crate::acp::AcpManager>,
    pub output_dir: PathBuf,
    pub attachments_dir: PathBuf,
    turns: Mutex<HashMap<String, CancellationToken>>,
    pending_wakes: Mutex<HashSet<String>>,
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
            acp: Arc::new(crate::acp::AcpManager::new()),
            output_dir,
            attachments_dir,
            turns: Mutex::new(HashMap::new()),
            pending_wakes: Mutex::new(HashSet::new()),
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
    #[allow(clippy::too_many_arguments)]
    pub async fn launch_subagent(
        self: &Arc<Self>,
        parent_id: &str,
        description: &str,
        prompt: &str,
        session_id: Option<&str>,
        agent: Option<&str>,
        part_id: &str,
    ) -> Result<(String, String)> {
        let parent = conversations::get(&self.db.pool, parent_id).await?;
        // An ACP child follows the same lifecycle as an internal one; only the
        // engine that runs its turns differs.
        if let Some(slug) = agent {
            return self
                .launch_acp_subagent(&parent, description, prompt, session_id, slug, part_id)
                .await;
        }

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
        // The parent owns the step budget. An unlimited parent (0) gives the
        // child the same unlimited budget; an explicit limit is inherited.
        conversations::set_max_steps(&self.db.pool, &child.id, parent.max_steps).await?;
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

    /// The slash commands the external agent advertised for this chat.
    ///
    /// Empty for an internal conversation, and empty before the agent's first
    /// session exists, which is why the composer also refreshes after a turn.
    pub async fn agent_commands(&self, conversation_id: &str) -> Vec<crate::acp::AgentCommand> {
        match self.acp.get(conversation_id).await {
            Some(session) => session.commands(),
            None => Vec::new(),
        }
    }

    /// The agent's configurable options for this chat: model, mode, effort.
    pub async fn agent_config(&self, conversation_id: &str) -> Vec<crate::acp::ConfigOption> {
        match self.acp.get(conversation_id).await {
            Some(session) => session.config_options(),
            None => Vec::new(),
        }
    }

    /// Ask the agent to change one option. The agent answers with the full
    /// updated list, which the next `agent_config` call returns.
    pub async fn set_agent_config(
        &self,
        conversation_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<crate::acp::ConfigOption>> {
        let session =
            self.acp.get(conversation_id).await.ok_or_else(|| {
                CoreError::Message("This chat has no live ACP session yet.".into())
            })?;
        session
            .set_config(config_id, value)
            .map_err(|error| CoreError::Message(error.user_message()))?;
        // The answer arrives asynchronously; give the pump a moment to apply it
        // so the caller can return the new list instead of the old one.
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            let now = session.config_options();
            if now
                .iter()
                .any(|option| option.id == config_id && option.current_value == value)
            {
                return Ok(now);
            }
        }
        Ok(session.config_options())
    }

    /// Start an agent, initialize it, open a session, then close it.
    ///
    /// Used by the settings probe. A wrong flag, a missing binary, and an
    /// unauthenticated agent all look identical in the config, so the only
    /// honest check is to speak the protocol once.
    pub async fn probe_acp_agent(
        agent: &crate::models::AcpAgent,
        workspace: &std::path::Path,
    ) -> Result<crate::acp::AcpProbe> {
        use crate::acp::session::PermissionDecision;

        let args = acp_agents::agent_args(agent)?;
        let env = acp_agents::agent_env(agent)?;
        let no_permissions: crate::acp::PermissionHandler = Arc::new(|_| {
            // A probe must never approve anything: decline and let the agent
            // finish the handshake.
            Box::pin(async { PermissionDecision::Cancel })
        });
        let (events_tx, _events_rx) = tokio::sync::mpsc::unbounded_channel();
        let session = crate::acp::session::open(
            crate::acp::session::SessionSpec {
                local_id: "probe".into(),
                command: &agent.command,
                args: &args,
                env: &env,
                cwd: workspace,
                resume: None,
                permissions: no_permissions,
            },
            events_tx,
        )
        .await;

        match session {
            Ok(session) => {
                let (name, version) = match &session.agent_info {
                    Some((name, version)) => (Some(name.clone()), Some(version.clone())),
                    None => (None, None),
                };
                let session_id = session.acp_session_id.clone();
                // Closing is best effort: the handle is dropped either way, and
                // the pump terminates the process.
                let _ = session.close();
                Ok(crate::acp::AcpProbe {
                    ok: true,
                    message: match (&name, &version) {
                        (Some(name), Some(version)) => {
                            format!("Connected to {name} {version}.")
                        }
                        _ => "Connected successfully.".to_owned(),
                    },
                    session_id: Some(session_id),
                    agent_name: name,
                    agent_version: version,
                })
            }
            Err(error) => Err(CoreError::Message(error.user_message())),
        }
    }

    /// Bind a conversation to an ACP agent, or clear the binding.
    ///
    /// The agent then runs every turn of that conversation instead of the LLM
    /// loop. Its own history is authoritative, so Jarvis does not send the
    /// transcript: only the newest user message is forwarded each turn.
    pub async fn set_conversation_agent(
        self: &Arc<Self>,
        conversation_id: &str,
        slug: Option<&str>,
    ) -> Result<()> {
        if self.in_flight(conversation_id) {
            return Err(CoreError::Message(
                "Stop the current turn before changing the agent.".into(),
            ));
        }
        let Some(slug) = slug.map(str::trim).filter(|value| !value.is_empty()) else {
            // Unbinding mid-conversation keeps the live session until it ends;
            // the next turn simply goes back to the LLM.
            conversations::set_acp(&self.db.pool, conversation_id, "", None).await?;
            if let Some(session) = self.acp.get(conversation_id).await {
                let _ = session.close();
            }
            self.acp.close(conversation_id).await.ok();
            let _ = self.events.send(CoreEvent::ConversationUpdated {
                conversation_id: conversation_id.to_owned(),
            });
            return Ok(());
        };
        let agent = acp_agents::get_by_slug(&self.db.pool, slug)
            .await?
            .ok_or_else(|| {
                CoreError::Message(format!(
                    "No ACP agent is registered with the slug {slug:?}."
                ))
            })?;
        if !agent.enabled {
            return Err(CoreError::Message(format!(
                "The ACP agent {slug:?} is disabled."
            )));
        }
        conversations::set_acp(&self.db.pool, conversation_id, &agent.slug, None).await?;

        // Tell the UI immediately. Opening the agent takes seconds — a real
        // Antigravity session takes 1.5 to 2.4 just to answer `session/new` —
        // and making the switch wait for that felt like a hang for a purely
        // visual change.
        let _ = self.events.send(CoreEvent::ConversationUpdated {
            conversation_id: conversation_id.to_owned(),
        });

        // Open the session in the background. The options still arrive without
        // the first message: the pump announces them, and a second
        // `ConversationUpdated` makes the pickers ask again.
        let engine = self.clone();
        let conversation_id = conversation_id.to_owned();
        tokio::spawn(async move {
            let workspace = match crate::agent::tools::workspace_root_for_conversation(
                &engine.db.pool,
                &conversation_id,
            )
            .await
            {
                Ok(workspace) => workspace,
                Err(error) => {
                    tracing::warn!(
                        conversation = %conversation_id,
                        error = %error.user_message(),
                        "could not resolve the workspace for the ACP session"
                    );
                    return;
                }
            };
            let policy =
                crate::db::repo::acp_agents::PermissionPolicy::parse(&agent.permission_policy)
                    .unwrap_or(crate::db::repo::acp_agents::PermissionPolicy::Ask);
            match acp_turn::ensure_session(
                engine.clone(),
                &conversation_id,
                &agent,
                &workspace,
                &tokio_util::sync::CancellationToken::new(),
                policy,
            )
            .await
            {
                Ok(active) => {
                    engine.acp.put_events(&conversation_id, active.events).await;
                    // The pickers load on this event; without it they would
                    // wait for the first turn, which is what felt broken.
                    let _ = engine.events.send(CoreEvent::ConversationUpdated {
                        conversation_id: conversation_id.clone(),
                    });
                }
                Err(error) => {
                    tracing::warn!(
                        conversation = %conversation_id,
                        error = %error.user_message(),
                        "could not open the ACP session while binding"
                    );
                }
            }
        });

        Ok(())
    }

    /// Launch a child backed by an external ACP agent.
    ///
    /// Mirrors [`Self::launch_subagent`] so the parent sees one kind of report,
    /// but records the ACP marker and never inherits provider/model settings:
    /// the agent owns its own model, tools, and credentials.
    async fn launch_acp_subagent(
        self: &Arc<Self>,
        parent: &Conversation,
        description: &str,
        prompt: &str,
        session_id: Option<&str>,
        slug: &str,
        part_id: &str,
    ) -> Result<(String, String)> {
        let agent = acp_agents::get_by_slug(&self.db.pool, slug).await?.ok_or_else(|| {
            CoreError::Message(format!(
                "No ACP agent is registered with the slug {slug:?}. Check Settings > ACP agents."
            ))
        })?;
        if !agent.enabled {
            return Err(CoreError::Message(format!(
                "The ACP agent {slug:?} is disabled."
            )));
        }

        if let Some(session_id) = session_id {
            let child = conversations::get(&self.db.pool, session_id).await?;
            if child.parent_id.as_deref() != Some(&parent.id) {
                return Err(CoreError::Message(
                    "That conversation is not a child of this chat.".into(),
                ));
            }
            if child.acp_agent.as_deref() != Some(slug) {
                return Err(CoreError::Message(format!(
                    "That subagent is not backed by the ACP agent {slug:?}."
                )));
            }
            self.mark_subagent_part(&parent.id, part_id, &child.id)
                .await;
            self.append_subagent_prompt(&child.id, prompt, false)
                .await?;
            let running = self.in_flight(&child.id);
            if !running {
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
                    "Follow-up sent to the running ACP subagent (sessionID: {}). It will report again when this round finishes.",
                    child.id
                )
            } else {
                format!(
                    "Follow-up sent. The ACP subagent is running in the background (sessionID: {}).",
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
        let child = conversations::create(&self.db.pool, &title, None, None).await?;
        conversations::set_parent(&self.db.pool, &child.id, &parent.id).await?;
        // ACP gets the parent's workspace, and the agent inherits nothing else:
        // its own config decides the model, tools, and credentials.
        conversations::set_workspace(&self.db.pool, &child.id, parent.workspace_root.as_deref())
            .await?;
        conversations::set_acp(&self.db.pool, &child.id, &agent.slug, None).await?;
        self.mark_subagent_part(&parent.id, part_id, &child.id)
            .await;
        self.append_subagent_prompt(&child.id, prompt, true).await?;
        let done = self.start_turn_with_done(&child.id)?;
        self.spawn_subagent_supervisor(
            parent.id.clone(),
            child.id.clone(),
            child.title.clone(),
            done,
        );
        let content = format!(
            "The ACP subagent is running in the background (sessionID: {}). You will receive a <subagent> report when it goes idle. Do not poll or duplicate its work; work on non-overlapping tasks or briefly tell the user and end your response.",
            child.id
        );
        Ok((child.id, content))
    }

    /// Run one ACP turn for `conversation_id`.
    async fn run_acp_turn(
        self: &Arc<Self>,
        conversation_id: &str,
        slug: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        let agent = match acp_agents::get_by_slug(&self.db.pool, slug).await? {
            Some(agent) if agent.enabled => agent,
            Some(_) => {
                let message = format!("The ACP agent {slug:?} is disabled.");
                self.fail_acp_turn(conversation_id, &message).await;
                return Ok(());
            }
            None => {
                // The row can be renamed or deleted after a chat is bound to it;
                // say so instead of silently falling back to the LLM.
                let message = format!(
                    "The ACP agent bound to this chat ({slug:?}) no longer exists. Pick another agent, or switch back to the built-in model."
                );
                self.fail_acp_turn(conversation_id, &message).await;
                return Ok(());
            }
        };

        // Only the newest user message is forwarded. The agent owns the rest of
        // the conversation, so sending Jarvis history would duplicate it.
        let history = messages::list_for_conversation(&self.db.pool, conversation_id).await?;
        let Some(last_user) = history
            .iter()
            .rev()
            .find(|item| item.message.role == "user")
        else {
            return Ok(());
        };
        let prompt = crate::agent::context::text_parts(&last_user.parts);
        let contexts = Self::message_contexts(&last_user.parts);
        let attachments = self.message_attachments(&last_user.parts);
        if prompt.trim().is_empty() && attachments.is_empty() && contexts.is_empty() {
            return Ok(());
        }

        let workspace =
            crate::agent::tools::workspace_root_for_conversation(&self.db.pool, conversation_id)
                .await?;
        acp_turn::run_turn(
            self.clone(),
            acp_turn::TurnSpec {
                child_id: conversation_id,
                prompt: &prompt,
                attachments,
                contexts,
                agent: &agent,
                cwd: &workspace,
            },
            cancel,
        )
        .await
    }

    /// Record a failure that happened before any transcript existed, so the
    /// chat does not sit on a user message with no answer.
    async fn fail_acp_turn(&self, conversation_id: &str, message: &str) {
        if let Ok(assistant) =
            messages::create(&self.db.pool, conversation_id, "assistant", "error").await
        {
            let _ = messages::append_part(
                &self.db.pool,
                &assistant.id,
                "text",
                &PartPayload::Text {
                    text: message.to_owned(),
                },
            )
            .await;
            let _ =
                messages::set_status(&self.db.pool, &assistant.id, "error", Some(message)).await;
            let _ = self.events.send(CoreEvent::MessageEnd {
                conversation_id: conversation_id.to_owned(),
                message_id: assistant.id,
                status: "error".into(),
                error: Some(message.to_owned()),
            });
        }
        let _ = self.events.send(CoreEvent::Status {
            conversation_id: conversation_id.to_owned(),
            status: crate::events::TurnStatus::Idle,
            message: Some(message.to_owned()),
        });
    }

    /// File contexts attached to a user message, as ACP needs them.
    fn message_contexts(parts: &[crate::models::Part]) -> Vec<MessageContext> {
        parts
            .iter()
            .filter(|part| part.kind == "context")
            .filter_map(|part| {
                let payload = part.payload();
                Some(MessageContext {
                    path: payload.get("path")?.as_str()?.to_owned(),
                    start_line: payload.get("startLine").and_then(serde_json::Value::as_i64),
                    end_line: payload.get("endLine").and_then(serde_json::Value::as_i64),
                    comment: payload.get("comment")?.as_str()?.to_owned(),
                    preview: payload
                        .get("preview")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                })
            })
            .collect()
    }

    /// Attachments on a user message, reduced to sendable ACP inputs.
    ///
    /// An image is read as base64 here; a file that cannot be read is skipped
    /// rather than sent empty.
    fn message_attachments(
        &self,
        parts: &[crate::models::Part],
    ) -> Vec<crate::acp::prompt::AttachmentInput> {
        parts
            .iter()
            .filter(|part| part.kind == "attachment")
            .filter_map(|part| {
                crate::acp::prompt::attachment_input(part, &|path| match std::fs::read(path) {
                    Ok(bytes) => {
                        use base64::Engine;
                        Some(base64::engine::general_purpose::STANDARD.encode(bytes))
                    }
                    Err(error) => {
                        tracing::debug!(path, %error, "could not read an attachment for ACP");
                        None
                    }
                })
            })
            .collect()
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
        let conversation_is_acp = conversations::get(&self.db.pool, &child_id)
            .await
            .ok()
            .and_then(|conversation| conversation.acp_agent)
            .is_some();
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
            // An ACP child reports only the text after its last tool call; an
            // internal one reports its final message as a whole.
            let (state, report) = if conversation_is_acp {
                acp_turn::report_from_history(&history)
            } else {
                let (state, report) = subagent_report(&history);
                (state.to_owned(), report)
            };
            self.finish_subagent(&parent_id, &child_id, &description, &state, &report)
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
        self.queue_subagent_wake(parent_id);
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
        self.send_message_with_contexts(conversation_id, text, attachment_ids, &[])
            .await
    }

    pub async fn send_message_with_contexts(
        self: &Arc<Self>,
        conversation_id: &str,
        text: &str,
        attachment_ids: &[String],
        contexts: &[MessageContext],
    ) -> Result<Message> {
        let text = text.trim();
        if text.is_empty() && attachment_ids.is_empty() && contexts.is_empty() {
            return Err(CoreError::invalid(
                "message has no text, attachments or context",
            ));
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
        if contexts.len() > 12 {
            return Err(CoreError::invalid(
                "Attach at most 12 file contexts at once.",
            ));
        }
        if contexts.iter().any(|context| {
            context.path.trim().is_empty()
                || context.comment.chars().count() > 8_000
                || context
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.chars().count() > 12_000)
        }) {
            return Err(CoreError::invalid("File context is invalid or too large."));
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
        for context in contexts {
            messages::append_part(
                &self.db.pool,
                &message.id,
                "context",
                &PartPayload::Context {
                    path: context.path.clone(),
                    start_line: context.start_line,
                    end_line: context.end_line,
                    comment: context.comment.clone(),
                    preview: context.preview.clone(),
                },
            )
            .await?;
        }
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
                contexts
                    .first()
                    .map(|context| context.path.as_str())
                    .or_else(|| {
                        attachments
                            .first()
                            .map(|attachment| attachment.name.as_str())
                    })
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
        self.steer_message_with_contexts(conversation_id, text, attachment_ids, &[])
            .await
    }

    pub async fn steer_message_with_contexts(
        self: &Arc<Self>,
        conversation_id: &str,
        text: &str,
        attachment_ids: &[String],
        contexts: &[MessageContext],
    ) -> Result<Message> {
        if !self.in_flight(conversation_id) {
            return self
                .send_message_with_contexts(conversation_id, text, attachment_ids, contexts)
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
        self.send_message_with_contexts(conversation_id, text, attachment_ids, contexts)
            .await
    }

    pub async fn regenerate(self: &Arc<Self>, conversation_id: &str) -> Result<()> {
        self.reject_acp_history_edit(conversation_id).await?;
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
        self.reject_acp_history_edit(conversation_id).await?;
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
        let contexts = Self::message_contexts(&target.parts);
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
        if text.trim().is_empty() && attachment_paths.is_empty() && contexts.is_empty() {
            return Err(CoreError::invalid(
                "message has no text, attachments or context to revert",
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
            contexts,
        })
    }

    /// Reject an operation that an ACP conversation cannot support.
    ///
    /// A normal message is fine: the agent runs the prompt itself, and
    /// `start_turn_with_done` routes the turn to it. Regenerating or continuing
    /// is not, because those rewrite Jarvis history the agent owns. Callers
    /// check this *before* writing rows, or a rejected request would leave a
    /// stray message behind.
    async fn reject_acp_history_edit(&self, conversation_id: &str) -> Result<()> {
        if let Ok(conversation) = conversations::get(&self.db.pool, conversation_id).await {
            if conversation.acp_agent.is_some() {
                return Err(CoreError::Message(
                    "This conversation is run by an ACP agent, which keeps its own history. Regenerate and continue are not available here."
                        .into(),
                ));
            }
        }
        Ok(())
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

    /// Queue a subagent report wake. If the parent is already idle, start the
    /// turn now; otherwise hold it until the current parent turn releases its
    /// slot and `wake_if_pending` runs.
    fn queue_subagent_wake(self: &Arc<Self>, parent_id: &str) {
        if self.in_flight(parent_id) {
            self.pending_wakes
                .lock()
                .expect("pending wakes lock")
                .insert(parent_id.to_owned());
            return;
        }
        if self.start_turn(parent_id).is_err() {
            self.pending_wakes
                .lock()
                .expect("pending wakes lock")
                .insert(parent_id.to_owned());
        }
    }

    fn wake_if_pending(self: &Arc<Self>, conversation_id: &str) {
        let pending = self
            .pending_wakes
            .lock()
            .expect("pending wakes lock")
            .remove(conversation_id);
        if !pending {
            return;
        }
        if self.in_flight(conversation_id) {
            self.pending_wakes
                .lock()
                .expect("pending wakes lock")
                .insert(conversation_id.to_owned());
            return;
        }
        if self.start_turn(conversation_id).is_err() {
            self.pending_wakes
                .lock()
                .expect("pending wakes lock")
                .insert(conversation_id.to_owned());
        }
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
            {
                // Drop the slot before checking for a queued wake, otherwise
                // the new turn would see this one as still in flight.
                let _slot = slot;
                // A child backed by an external ACP agent must never run the
                // LLM loop: the agent owns its own model and tools.
                let acp_agent = conversations::get(&engine.db.pool, &id)
                    .await
                    .ok()
                    .and_then(|conversation| conversation.acp_agent);
                let result = match acp_agent {
                    Some(slug) => engine.run_acp_turn(&id, &slug, token).await,
                    None => runner::run(engine.clone(), &id, token).await,
                };
                if let Err(error) = result {
                    tracing::debug!(conversation = %id, error = %error.user_message(), "turn ended with error");
                }
            }
            let _ = done_tx.send(());
            engine.wake_if_pending(&id);
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
        agent: Option<&'a str>,
        part_id: &'a str,
    ) -> BoxFuture<'a, Result<(String, String)>> {
        Box::pin(async move {
            let engine = self.engine.upgrade().ok_or_else(|| {
                CoreError::Message("The subagent runner is not available.".into())
            })?;
            engine
                .launch_subagent(parent_id, description, prompt, session_id, agent, part_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::db::repo::conversations;
    use crate::mcp::McpManager;
    use crate::secrets::MemoryStore;

    #[tokio::test]
    async fn a_queued_subagent_wake_starts_after_the_parent_turn_releases_its_slot() {
        let db = Db::open_memory().await.unwrap();
        let conversation = conversations::create(&db.pool, "Parent", None, None)
            .await
            .unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let mcp = Arc::new(McpManager::new(db.clone(), secrets.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let engine = ChatEngine::new(
            db,
            secrets,
            mcp,
            tmp.path().join("tool-output"),
            tmp.path().join("attachments"),
        );
        let (slot, _token) = engine
            .claim_turn(&conversation.id, "parent is busy")
            .unwrap();

        engine.queue_subagent_wake(&conversation.id);
        assert!(
            engine
                .pending_wakes
                .lock()
                .expect("pending wakes lock")
                .contains(&conversation.id)
        );

        drop(slot);
        engine.wake_if_pending(&conversation.id);
        assert!(
            !engine
                .pending_wakes
                .lock()
                .expect("pending wakes lock")
                .contains(&conversation.id)
        );
    }
}
