//! The `question` tool.

use super::{Rendered, ToolExecRequest, ToolExecutor};
use crate::agent::approvals::InputAnswer;
use crate::events::CoreEvent;
use crate::{CoreError, Result};
use serde_json::{Value, json};

impl ToolExecutor {
    pub(super) async fn run_question(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let questions = req
            .args
            .get("questions")
            .cloned()
            .ok_or_else(|| CoreError::invalid("questions is required"))?;
        let _ = self.events.send(CoreEvent::InputRequest {
            conversation_id: req.conversation_id.clone(),
            call_id: req.call_id.clone(),
            request: json!({"questions": questions}),
        });
        let receiver = self.inputs.register(&req.call_id);
        let answer = tokio::select! {
            _ = req.cancel.cancelled() => {
                self.inputs.cancel(&req.call_id);
                return Err(CoreError::Cancelled);
            }
            answer = receiver => answer.ok(),
        };
        let Some(InputAnswer {
            responses,
            cancelled,
        }) = answer
        else {
            return Err(CoreError::Message(
                "The question was closed without an answer.".into(),
            ));
        };
        if cancelled {
            return Err(CoreError::Message(
                "The user cancelled the question.".into(),
            ));
        }
        let answers = responses.get("answers").cloned().unwrap_or(responses);
        let mut content = String::from("The user answered:\n");
        if let Some(items) = answers.as_array() {
            for item in items {
                let header = item
                    .get("header")
                    .and_then(Value::as_str)
                    .unwrap_or("Question");
                let question = item.get("question").and_then(Value::as_str).unwrap_or("");
                let answer = item.get("answer").and_then(Value::as_str).unwrap_or("");
                content.push_str(&format!("- {header}: {answer}\n"));
                if !question.is_empty() {
                    content.push_str(&format!("  ({question})\n"));
                }
            }
        } else {
            content.push_str(&serde_json::to_string_pretty(&answers).unwrap_or_default());
            content.push('\n');
        }
        Ok(Rendered {
            content,
            structured: Some(answers),
            ok: true,
        })
    }
}
