use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_MAX_STEPS: i64 = 0;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: String,
    pub parent_id: Option<String>,
    pub workspace_root: Option<String>,
    pub system_prompt: Option<String>,
    pub archived: bool,
    pub pinned: bool,
    pub max_steps: i64,
    pub tool_mode: String,
    pub servers_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub created_at: i64,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub id: String,
    pub message_id: String,
    pub idx: i64,
    pub kind: String,
    pub payload_json: String,
}

impl Part {
    pub fn payload(&self) -> Value {
        serde_json::from_str(&self.payload_json).unwrap_or(Value::Null)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageWithParts {
    #[serde(flatten)]
    pub message: Message,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub messages: Vec<MessageWithParts>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenaiCompat,
    Anthropic,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCompat => "openai_compat",
            Self::Anthropic => "anthropic",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_compat" | "openai" => Some(Self::OpenaiCompat),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub model: String,
    pub api_key_ref: Option<String>,
    pub context_window: i64,
    pub max_output_tokens: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub models_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    #[serde(flatten)]
    pub provider: Provider,
    pub has_key: bool,
    pub key_hint: Option<String>,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerPolicy {
    Auto,
    Ask,
    Deny,
}

impl ServerPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    None,
    Bearer,
    ApiKey,
    Oauth,
}

impl AuthKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bearer => "bearer",
            Self::ApiKey => "api_key",
            Self::Oauth => "oauth",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "bearer" => Some(Self::Bearer),
            "api_key" => Some(Self::ApiKey),
            "oauth" => Some(Self::Oauth),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub url: String,
    pub transport: String,
    pub command: String,
    pub args_json: String,
    pub env_json: String,
    pub cwd: String,
    pub auth_kind: String,
    pub headers_json: String,
    pub enabled: bool,
    pub policy: String,
    pub protocol_version: Option<String>,
    pub created_at: i64,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_checked_at: Option<i64>,
    pub server_info_json: Option<String>,
    pub capabilities_json: Option<String>,
    pub tools_json: Option<String>,
    pub tools_cached_at: Option<i64>,
    pub oauth_issuer: Option<String>,
    pub oauth_client_id: Option<String>,
    pub negotiated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    #[serde(flatten)]
    pub server: Server,
    pub has_bearer: bool,
    pub has_api_key: bool,
    pub has_oauth_token: bool,
    pub tool_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRule {
    pub id: String,
    pub server_id: Option<String>,
    pub action: String,
    pub resource: String,
    pub effect: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub conversation_id: String,
    pub title: String,
    pub snippet: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

impl Usage {
    /// Accepts both OpenAI (`prompt_tokens`/`completion_tokens`) and Anthropic
    /// (`input_tokens`/`output_tokens`) shapes.
    pub fn from_json(value: &Value) -> Option<Self> {
        let prompt = value
            .get("prompt_tokens")
            .or_else(|| value.get("input_tokens"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let completion = value
            .get("completion_tokens")
            .or_else(|| value.get("output_tokens"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if prompt == 0 && completion == 0 {
            None
        } else {
            Some(Self {
                prompt_tokens: prompt,
                completion_tokens: completion,
            })
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PartPayload {
    Text {
        text: String,
    },
    Attachment {
        id: String,
        name: String,
        mime: String,
        size: i64,
        file_kind: String,
        path: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        truncated: bool,
    },
    Reasoning {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        server: String,
        args: Value,
        state: String,
        started_at: Option<i64>,
        ended_at: Option<i64>,
        executed: bool,
        approval: Option<String>,
        error: Option<String>,
        #[serde(default)]
        result_preview: Option<String>,
        #[serde(default)]
        full_result_ref: Option<String>,
        #[serde(default)]
        truncated: bool,
    },
    ToolResult {
        call_id: String,
        name: String,
        server: String,
        ok: bool,
        content: String,
        structured: Option<Value>,
        full_result_ref: Option<String>,
        truncated: bool,
    },
    Error {
        message: String,
    },
    Summary {
        text: String,
        covers_until: String,
    },
}
