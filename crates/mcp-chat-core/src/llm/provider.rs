use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::Result;
use crate::models::Usage;

pub type DeltaStream = Pin<Box<dyn Stream<Item = Result<Delta>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Unknown(String),
}

#[derive(Debug, Clone)]
pub enum Delta {
    Text(String),
    Reasoning(String),
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    ToolCallArgs {
        index: usize,
        id: String,
        fragment: String,
    },
    ToolCallEnd {
        index: usize,
        id: String,
    },
    Usage(Usage),
    Done(FinishReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    None,
}

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub kind: crate::models::ProviderKind,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDef>,
    pub tool_choice: ToolChoice,
    pub max_tokens: i64,
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
    ToolResult {
        call_id: String,
        name: String,
        content: String,
        is_error: bool,
    },
}

impl ChatMessage {
    pub fn text(role: ChatRole, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentBlock::Text(text.into())],
        }
    }

    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_calls(&self) -> Vec<(String, String, Value)> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                } => Some((id.clone(), name.clone(), arguments.clone())),
                _ => None,
            })
            .collect()
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    async fn stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<DeltaStream>;
}

pub fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("mcp-chat/{}", crate::APP_VERSION))
        .build()?)
}

pub fn openai_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_owned()
    } else {
        format!("{base}/chat/completions")
    }
}

pub fn anthropic_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/messages") {
        base.to_owned()
    } else if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

pub fn models_url(kind: crate::models::ProviderKind, base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    match kind {
        crate::models::ProviderKind::OpenaiCompat => format!("{base}/models"),
        crate::models::ProviderKind::Anthropic => {
            if base.ends_with("/v1") {
                format!("{base}/models")
            } else {
                format!("{base}/v1/models")
            }
        }
    }
}

pub fn parse_finish_reason(raw: &str) -> FinishReason {
    match raw {
        "stop" | "end_turn" | "stop_sequence" => FinishReason::Stop,
        "tool_calls" | "tool_use" | "function_call" => FinishReason::ToolCalls,
        "length" | "max_tokens" => FinishReason::Length,
        "content_filter" | "refusal" => FinishReason::ContentFilter,
        other => FinishReason::Unknown(other.to_owned()),
    }
}

pub async fn response_error(response: reqwest::Response) -> crate::CoreError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let body = body.chars().take(2000).collect::<String>();
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
        })
        .unwrap_or_else(|| body.trim().to_owned());
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return crate::CoreError::Message(if message.is_empty() {
            "Authentication failed. Check the API key.".into()
        } else {
            format!("Authentication failed: {message}")
        });
    }
    if crate::error::is_retryable_status(status) {
        return crate::CoreError::Message(format!(
            "Provider is temporarily unavailable (HTTP {status}). {}",
            message
        ));
    }
    if crate::error::is_context_overflow_message(&message) {
        return crate::CoreError::ContextOverflow;
    }
    crate::CoreError::Message(format!(
        "Provider returned HTTP {status}: {}",
        if message.is_empty() {
            "no details"
        } else {
            &message
        }
    ))
}
