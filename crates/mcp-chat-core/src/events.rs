use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Events emitted by the core and forwarded to the webview by the Tauri shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CoreEvent {
    Status {
        conversation_id: String,
        status: TurnStatus,
        message: Option<String>,
    },
    MessageStart {
        conversation_id: String,
        message_id: String,
        /// `assistant` for normal turns, `system` for compaction summaries.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
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
        /// UI-only unified diff for built-in file mutations. It is also stored
        /// on the tool_call part and is never sent to the model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
    },
    Approval {
        conversation_id: String,
        call_id: String,
        server: String,
        tool: String,
        args: Value,
        allow_always: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
    },
    Usage {
        conversation_id: String,
        prompt_tokens: i64,
        completion_tokens: i64,
        context_window: i64,
    },
    Step {
        conversation_id: String,
        step: i64,
        max_steps: i64,
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
    Resync {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Thinking,
    Streaming,
    Compacting,
    WaitingApproval,
    RunningTool,
    Idle,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_event_carries_a_diff_only_when_present() {
        let mut event = CoreEvent::Tool {
            conversation_id: "c1".into(),
            call_id: "call1".into(),
            state: "completed".into(),
            progress: None,
            message: None,
            diff: None,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert!(value.get("diff").is_none());
        if let CoreEvent::Tool { diff, .. } = &mut event {
            *diff = Some("-old\n+new\n".into());
        }
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["diff"], "-old\n+new\n");
        // Events without the field still deserialize.
        let legacy: CoreEvent = serde_json::from_value(
            json!({"type": "tool", "conversationId": "c1", "callId": "x", "state": "running", "progress": null, "message": null}),
        )
        .unwrap();
        assert!(matches!(legacy, CoreEvent::Tool { diff: None, .. }));
    }

    #[test]
    fn event_fields_are_camel_case_for_the_webview() {
        let delta = CoreEvent::Delta {
            conversation_id: "c1".into(),
            message_id: "m1".into(),
            part_id: "p1".into(),
            delta_type: "text".into(),
            text: "hello".into(),
        };
        let value = serde_json::to_value(delta).unwrap();
        assert_eq!(value["type"], "delta");
        assert_eq!(value["conversationId"], "c1");
        assert_eq!(value["messageId"], "m1");
        assert_eq!(value["partId"], "p1");
        assert_eq!(value["deltaType"], "text");

        let start = CoreEvent::MessageStart {
            conversation_id: "c1".into(),
            message_id: "m1".into(),
            role: Some("system".into()),
        };
        let value = serde_json::to_value(start).unwrap();
        assert_eq!(value["role"], "system");

        let approval = CoreEvent::Approval {
            conversation_id: "c1".into(),
            call_id: "call1".into(),
            server: "Mock".into(),
            tool: "echo".into(),
            args: json!({"text": "hi"}),
            allow_always: true,
            diff: None,
        };
        let value = serde_json::to_value(approval).unwrap();
        assert_eq!(value["callId"], "call1");
        assert_eq!(value["conversationId"], "c1");
        assert_eq!(value["allowAlways"], true);

        let usage = CoreEvent::Usage {
            conversation_id: "c1".into(),
            prompt_tokens: 1200,
            completion_tokens: 34,
            context_window: 128_000,
        };
        let value = serde_json::to_value(usage).unwrap();
        assert_eq!(value["promptTokens"], 1200);
        assert_eq!(value["contextWindow"], 128_000);
    }
}
