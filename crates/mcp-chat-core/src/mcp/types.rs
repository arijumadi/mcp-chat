//! MCP JSON-RPC types shared by the 2026-07-28 and legacy clients.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(default)]
    pub tools: Option<Value>,
    #[serde(default)]
    pub prompts: Option<Value>,
    #[serde(default)]
    pub resources: Option<Value>,
    #[serde(default)]
    pub logging: Option<Value>,
    #[serde(default)]
    pub completions: Option<Value>,
    #[serde(default)]
    pub elicitation: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default = "empty_schema")]
    pub input_schema: Value,
    #[serde(default)]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub annotations: Option<Value>,
}

fn empty_schema() -> Value {
    serde_json::json!({"type": "object", "properties": {}})
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    #[serde(default)]
    pub tools: Vec<ToolInfo>,
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub ttl_ms: Option<i64>,
    #[serde(default)]
    pub cache_scope: Option<String>,
    #[serde(default)]
    pub result_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(default)]
    pub structured_content: Option<Value>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub result_type: Option<String>,
    #[serde(default)]
    pub input_requests: Option<Value>,
    #[serde(default)]
    pub request_state: Option<Value>,
    #[serde(default)]
    pub ttl_ms: Option<i64>,
    #[serde(default)]
    pub cache_scope: Option<String>,
}

impl CallToolResult {
    pub fn result_type(&self) -> &str {
        self.result_type.as_deref().unwrap_or("complete")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("MCP error {code}: {message}")]
pub struct McpError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl From<JsonRpcError> for McpError {
    fn from(value: JsonRpcError) -> Self {
        Self {
            code: value.code,
            message: value.message,
            data: value.data,
        }
    }
}

impl McpError {
    pub fn humanized(&self) -> String {
        match self.code {
            -32020 => "The server rejected the request headers; this server may be behind a proxy that strips them.".into(),
            -32022 => format!(
                "The server does not support this MCP protocol version. {}",
                self.message
            ),
            -32602 => format!("Invalid tool input: {}", self.message),
            -32601 => "The server does not implement this method.".into(),
            -32021 => "The server requires a client capability this app did not advertise.".into(),
            -32002 => "The requested resource was not found on the server.".into(),
            _ => format!("{} (code {})", self.message, self.code),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    StreamableHttp,
    LegacyHttp,
    Sse,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StreamableHttp => "streamable_http",
            Self::LegacyHttp => "legacy_http",
            Self::Sse => "sse",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "streamable_http" => Some(Self::StreamableHttp),
            "legacy_http" => Some(Self::LegacyHttp),
            "sse" => Some(Self::Sse),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverResult {
    #[serde(default)]
    pub supported_versions: Vec<String>,
    #[serde(default)]
    pub server_info: Option<ServerInfo>,
    #[serde(default)]
    pub capabilities: Option<ServerCapabilities>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub result_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Negotiated {
    pub protocol_version: String,
    pub server_info: Option<ServerInfo>,
    pub capabilities: Option<ServerCapabilities>,
    pub instructions: Option<String>,
    pub transport: TransportKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_result_type_is_complete() {
        let result: CallToolResult =
            serde_json::from_value(serde_json::json!({"content": []})).unwrap();
        assert_eq!(result.result_type(), "complete");
    }

    #[test]
    fn error_codes_are_humanized() {
        let error = McpError {
            code: -32020,
            message: "mismatch".into(),
            data: None,
        };
        assert!(error.humanized().contains("headers"));
    }
}
