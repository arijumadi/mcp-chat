use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Events emitted by the core and forwarded to the webview by the Tauri shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoreEvent {
    Status {
        conversation_id: String,
        status: TurnStatus,
        message: Option<String>,
    },
    MessageStart {
        conversation_id: String,
        message_id: String,
    },
    Delta {
        conversation_id: String,
        message_id: String,
        part_id: String,
        delta_type: String,
        text: String,
    },
    Part {
        conversation_id: String,
        message_id: String,
        part_id: String,
        kind: String,
        payload: Value,
    },
    MessageEnd {
        conversation_id: String,
        message_id: String,
        status: String,
        error: Option<String>,
    },
    Tool {
        conversation_id: String,
        call_id: String,
        state: String,
        progress: Option<f64>,
        message: Option<String>,
    },
    Approval {
        conversation_id: String,
        call_id: String,
        server: String,
        tool: String,
        args: Value,
    },
    InputRequest {
        conversation_id: String,
        call_id: String,
        request: Value,
    },
    ConversationUpdated {
        conversation_id: String,
    },
    Error {
        conversation_id: Option<String>,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Thinking,
    Streaming,
    WaitingApproval,
    RunningTool,
    Idle,
    Error,
}
