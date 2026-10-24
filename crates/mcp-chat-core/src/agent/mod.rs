pub mod approvals;
pub mod context;
pub mod permissions;
pub mod prompt;
pub mod runner;
pub mod tools;
pub mod types;

use std::collections::HashMap;
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

pub struct ChatEngine {
    pub db: Db,
    pub secrets: Arc<dyn SecretStore>,
    pub mcp: Arc<McpManager>,
    pub events: broadcast::Sender<CoreEvent>,
    pub approvals: Arc<ApprovalRegistry>,
    pub inputs: Arc<InputRegistry>,
    pub executor: Arc<ToolExecutor>,
    pub output_dir: PathBuf,
    turns: Mutex<HashMap<String, CancellationToken>>,
}

impl ChatEngine {
    pub fn new(
        db: Db,
        secrets: Arc<dyn SecretStore>,
        mcp: Arc<McpManager>,
        output_dir: PathBuf,
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
            turns: Mutex::new(HashMap::new()),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.events.subscribe()
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
        let text = text.trim();
        if text.is_empty() {
            return Err(CoreError::invalid("message is empty"));
        }
        if text.chars().count() > 100_000 {
            return Err(CoreError::invalid("message is too long"));
        }
        if self.in_flight(conversation_id) {
            return Err(CoreError::Message(
                "This conversation is already generating a response.".into(),
            ));
        }
        let conversation = conversations::get(&self.db.pool, conversation_id).await?;
        let message = messages::create(&self.db.pool, conversation_id, "user", "completed").await?;
        messages::append_part(
            &self.db.pool,
            &message.id,
            "text",
            &PartPayload::Text {
                text: text.to_owned(),
            },
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
        if conversation.title == "New chat" || conversation.title.trim().is_empty() {
            conversations::update_title(
                &self.db.pool,
                conversation_id,
                &conversations::title_from_text(text),
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

    pub async fn revert_message(&self, conversation_id: &str, message_id: &str) -> Result<String> {
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
        if text.trim().is_empty() {
            return Err(CoreError::invalid("message has no text to revert"));
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

        messages::delete_from(&self.db.pool, conversation_id, message_id).await?;
        let remaining = messages::list_for_conversation(&self.db.pool, conversation_id).await?;
        if remaining.is_empty() {
            conversations::update_title(&self.db.pool, conversation_id, "New chat").await?;
        }
        conversations::touch(&self.db.pool, conversation_id).await?;
        let _ = self.events.send(CoreEvent::ConversationUpdated {
            conversation_id: conversation_id.to_owned(),
        });
        Ok(text)
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
