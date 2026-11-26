//! One ACP-backed turn of a child conversation.
//!
//! This is where the isolated client meets the engine. It owns three things:
//!
//! - the transcript: [`AcpEvent`]s become `text`, `reasoning`, and `tool_call`
//!   parts on one assistant message, so the existing UI renders an ACP child
//!   exactly like an internal subagent;
//! - permissions: a request from the agent is forwarded to the same approval
//!   prompt the built-in tools use, and the answer picks a real ACP option;
//! - the report: the child history is what the parent receives.
//!
//! Everything here is deliberately engine-shaped. The protocol work stays in
//! [`crate::acp`]; this module only translates.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::AcpEvent;
use super::approvals::ApprovalDecision;
use super::session::{PermissionDecision, PermissionHandler, PermissionRequestInfo};
use crate::ChatEngine;
use crate::db::repo::{acp_agents, messages};
use crate::events::{CoreEvent, TurnStatus};
use crate::models::MessageWithParts;
use crate::models::PartPayload;
use crate::{CoreError, Result};

/// The text a parent sees when an ACP turn ends badly.
const NO_ANSWER: &str = "The ACP agent finished without a final answer.";

/// Everything a turn needs from the parent conversation.
pub struct TurnSpec<'a> {
    /// The child conversation that owns this ACP session.
    pub child_id: &'a str,
    /// The prompt text to send.
    pub prompt: &'a str,
    /// The registry row the child was configured with.
    pub agent: &'a crate::models::AcpAgent,
    /// Absolute working directory, already checked to be inside the parent's
    /// workspace.
    pub cwd: &'a std::path::Path,
}

/// Run one prompt turn against the agent, reusing its live session if any.
///
/// Output is written as it streams, so the child conversation is readable while
/// the turn runs. A turn failure is stored on the assistant message, which is
/// what [`report_from_history`] later turns into a failed report.
pub async fn run_turn(
    engine: Arc<ChatEngine>,
    spec: TurnSpec<'_>,
    cancel: CancellationToken,
) -> Result<()> {
    let args = acp_agents::agent_args(spec.agent)?;
    let env = acp_agents::agent_env(spec.agent)?;
    let handler = permission_handler(engine.clone(), spec.child_id, &cancel);

    let active = match engine.acp.take(spec.child_id).await {
        Some(active) => active,
        None => engine
            .acp
            .open(super::session::SessionSpec {
                local_id: spec.child_id.to_owned(),
                command: &spec.agent.command,
                args: &args,
                env: &env,
                cwd: spec.cwd,
                permissions: handler,
            })
            .await
            .map_err(|error| CoreError::Message(error.user_message()))?,
    };

    let result = stream_turn(
        engine.clone(),
        spec.child_id,
        spec.prompt,
        &active.handle,
        active.events,
        cancel,
    )
    .await;
    // Hand the stream back even when the turn failed: the session may still be
    // usable, and dropping the receiver would end the pump's send side.
    if let Ok(events) = result {
        engine.acp.put_events(spec.child_id, events).await;
    }
    Ok(())
}

struct TurnRun<'a> {
    engine: &'a ChatEngine,
    child_id: &'a str,
    events: mpsc::UnboundedReceiver<AcpEvent>,
    message_id: Option<String>,
    text: String,
    text_part: Option<String>,
    reasoning_part: Option<String>,
    tools: HashMap<String, String>,
    failure: Option<String>,
}

async fn stream_turn(
    engine: Arc<ChatEngine>,
    child_id: &str,
    prompt: &str,
    handle: &Arc<super::AcpSession>,
    events: mpsc::UnboundedReceiver<AcpEvent>,
    cancel: CancellationToken,
) -> Result<mpsc::UnboundedReceiver<AcpEvent>> {
    let mut run = TurnRun {
        engine: &engine,
        child_id,
        events,
        message_id: None,
        text: String::new(),
        text_part: None,
        reasoning_part: None,
        tools: HashMap::new(),
        failure: None,
    };

    if handle.prompt(prompt).is_err() {
        return Err(CoreError::Message(
            "That ACP session has ended. Start a new session.".into(),
        ));
    }
    let _ = engine.events.send(CoreEvent::Status {
        conversation_id: child_id.to_owned(),
        status: TurnStatus::Thinking,
        message: None,
    });

    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => {
                // The agent must be told, or it keeps working for nobody.
                let _ = handle.cancel();
                None
            }
            next = run.events.recv() => next,
        };
        let Some(event) = next else {
            break;
        };
        match event {
            AcpEvent::Text(chunk) => run.append_text(&chunk).await,
            AcpEvent::Thought(chunk) => run.append_reasoning(&chunk).await,
            AcpEvent::ToolCall {
                call_id,
                title,
                kind,
                status,
                detail,
            } => {
                run.upsert_tool(&call_id, &title, kind, &status, detail)
                    .await;
            }
            AcpEvent::Plan(_) | AcpEvent::PermissionRequested { .. } => {}
            AcpEvent::TurnEnded { .. } => break,
            AcpEvent::TurnFailed { message } => {
                run.failure = Some(message);
                break;
            }
        }
    }

    run.finish().await;
    Ok(run.events)
}

impl TurnRun<'_> {
    /// Create the assistant message on first output.
    async fn message(&mut self) -> Option<String> {
        if let Some(id) = &self.message_id {
            return Some(id.clone());
        }
        let message = messages::create(
            &self.engine.db.pool,
            self.child_id,
            "assistant",
            "streaming",
        )
        .await
        .ok()?;
        self.message_id = Some(message.id.clone());
        let _ = self.engine.events.send(CoreEvent::MessageStart {
            conversation_id: self.child_id.to_owned(),
            message_id: message.id.clone(),
            role: None,
        });
        Some(message.id)
    }

    async fn append_text(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let Some(message_id) = self.message().await else {
            return;
        };
        if self.text_part.is_none() {
            let part = messages::append_part(
                &self.engine.db.pool,
                &message_id,
                "text",
                &PartPayload::Text {
                    text: String::new(),
                },
            )
            .await;
            self.text_part = part.ok().map(|part| part.id);
        }
        let Some(part_id) = self.text_part.clone() else {
            return;
        };
        self.text.push_str(chunk);
        let _ = self.engine.events.send(CoreEvent::Delta {
            conversation_id: self.child_id.to_owned(),
            message_id,
            part_id: part_id.clone(),
            delta_type: "text".into(),
            text: chunk.to_owned(),
        });
        let _ = messages::update_part_text(&self.engine.db.pool, &part_id, &self.text).await;
    }

    async fn append_reasoning(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let Some(message_id) = self.message().await else {
            return;
        };
        if self.reasoning_part.is_none() {
            let part = messages::append_part(
                &self.engine.db.pool,
                &message_id,
                "reasoning",
                &PartPayload::Reasoning {
                    text: String::new(),
                },
            )
            .await;
            self.reasoning_part = part.ok().map(|part| part.id);
        }
        let Some(part_id) = self.reasoning_part.clone() else {
            return;
        };
        let _ = self.engine.events.send(CoreEvent::Delta {
            conversation_id: self.child_id.to_owned(),
            message_id,
            part_id: part_id.clone(),
            delta_type: "reasoning".into(),
            text: chunk.to_owned(),
        });
        let _ = messages::update_part_text(&self.engine.db.pool, &part_id, chunk).await;
    }

    async fn upsert_tool(
        &mut self,
        call_id: &str,
        title: &str,
        kind: Option<String>,
        status: &str,
        detail: Option<String>,
    ) {
        let Some(message_id) = self.message().await else {
            return;
        };
        // The UI's vocabulary, not the protocol's: `running`, `completed`,
        // `error`. `in_progress` and `pending` both mean a live row.
        let state = match status {
            "completed" => "completed",
            "failed" => "error",
            _ => "running",
        };

        if let Some(part_id) = self.tools.get(call_id).cloned() {
            let Ok(part) = messages::get_part(&self.engine.db.pool, &part_id).await else {
                return;
            };
            let mut payload = part.payload();
            payload["state"] = json!(state);
            if state != "running" {
                payload["endedAt"] = json!(crate::now_ms());
            }
            if let Some(detail) = detail {
                payload["resultPreview"] = json!(detail);
            }
            let _ = messages::update_part_payload(&self.engine.db.pool, &part_id, &payload).await;
            let _ = self.engine.events.send(CoreEvent::Part {
                conversation_id: self.child_id.to_owned(),
                message_id,
                part_id,
                kind: "tool_call".into(),
                payload,
            });
            return;
        }

        let mut payload = json!({
            "callId": call_id,
            "name": title,
            "server": "ACP",
            "args": {
                "title": title,
                "kind": kind,
            },
            "state": state,
            "startedAt": crate::now_ms(),
            "endedAt": if state == "running" { serde_json::Value::Null } else { json!(crate::now_ms()) },
            "executed": false,
            "approval": null,
        });
        if let Some(detail) = detail {
            payload["resultPreview"] = json!(detail);
        }
        let Ok(part) =
            messages::append_raw_part(&self.engine.db.pool, &message_id, "tool_call", &payload)
                .await
        else {
            return;
        };
        self.tools.insert(call_id.to_owned(), part.id.clone());
        let _ = self.engine.events.send(CoreEvent::Part {
            conversation_id: self.child_id.to_owned(),
            message_id,
            part_id: part.id,
            kind: "tool_call".into(),
            payload,
        });
    }

    /// Settle the message and its status.
    async fn finish(&mut self) {
        if let Some(message_id) = self.message_id.clone() {
            let (status, error) = match &self.failure {
                Some(message) => ("error", Some(message.clone())),
                None => ("completed", None),
            };
            let _ =
                messages::set_status(&self.engine.db.pool, &message_id, status, error.as_deref())
                    .await;
            let _ = self.engine.events.send(CoreEvent::MessageEnd {
                conversation_id: self.child_id.to_owned(),
                message_id,
                status: status.to_owned(),
                error,
            });
        }
        let _ = self.engine.events.send(CoreEvent::Status {
            conversation_id: self.child_id.to_owned(),
            status: TurnStatus::Idle,
            message: None,
        });
    }
}

/// Forward an ACP permission request to the normal approval prompt.
///
/// The answer maps onto a real ACP option: approve selects `allow_once`, reject
/// selects `reject_once`, and a dismissal cancels the turn. `allow_always` is
/// never offered, because Jarvis cannot scope a rule the agent owns.
fn permission_handler(
    engine: Arc<ChatEngine>,
    child_id: &str,
    cancel: &CancellationToken,
) -> PermissionHandler {
    let cancel = cancel.clone();
    Arc::new(move |info: PermissionRequestInfo| {
        let engine = engine.clone();
        let child_id = child_id.to_owned();
        let cancel = cancel.clone();
        Box::pin(async move {
            let call_id = format!("acp-{}", uuid::Uuid::new_v4());
            let _ = engine.events.send(CoreEvent::Approval {
                conversation_id: child_id.clone(),
                call_id: call_id.clone(),
                server: "ACP".into(),
                tool: info.title.clone(),
                args: json!({
                    "kind": info.kind,
                    "options": info
                        .options
                        .iter()
                        .map(|(id, _)| id.clone())
                        .collect::<Vec<_>>(),
                }),
                allow_always: false,
                action: info.kind.clone().unwrap_or_else(|| "tool".into()),
                resource: info.title.clone(),
                save_pattern: None,
                diff: None,
            });
            let receiver = engine.approvals.register(&call_id);
            let answer = tokio::select! {
                _ = cancel.cancelled() => {
                    engine.approvals.cancel(&call_id);
                    return PermissionDecision::Cancel;
                }
                answer = receiver => answer.ok(),
            };
            match answer.map(|answer| answer.decision) {
                Some(ApprovalDecision::Once | ApprovalDecision::Always) => {
                    PermissionDecision::Approve
                }
                Some(ApprovalDecision::Reject) => PermissionDecision::Reject,
                None => PermissionDecision::Cancel,
            }
        })
    })
}

/// Turn a finished child history into the parent-facing report.
pub fn report_from_history(history: &[MessageWithParts]) -> (String, String) {
    let Some(last) = history
        .iter()
        .rev()
        .find(|item| item.message.role == "assistant")
    else {
        return ("failed".to_owned(), NO_ANSWER.to_owned());
    };
    if last.message.status == "error" {
        return (
            "failed".to_owned(),
            last.message
                .error
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "The ACP agent failed.".to_owned()),
        );
    }
    let report = crate::agent::context::text_parts(&last.parts);
    let report = report.trim();
    if report.is_empty() {
        ("failed".to_owned(), NO_ANSWER.to_owned())
    } else {
        ("completed".to_owned(), report.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Message, Part};

    fn message(role: &str, status: &str, error: Option<&str>) -> Message {
        Message {
            id: "m1".into(),
            conversation_id: "c1".into(),
            role: role.into(),
            created_at: 0,
            status: status.into(),
            error: error.map(str::to_owned),
        }
    }

    fn text_part(text: &str) -> Part {
        Part {
            id: "p1".into(),
            message_id: "m1".into(),
            idx: 0,
            kind: "text".into(),
            payload_json: json!({ "text": text }).to_string(),
        }
    }

    fn history(message: Message, parts: Vec<Part>) -> Vec<MessageWithParts> {
        vec![MessageWithParts { message, parts }]
    }

    #[test]
    fn a_completed_turn_reports_its_text() {
        let (state, report) = report_from_history(&history(
            message("assistant", "completed", None),
            vec![text_part("All done.")],
        ));
        assert_eq!(state, "completed");
        assert_eq!(report, "All done.");
    }

    #[test]
    fn an_error_turn_reports_its_reason() {
        let (state, report) = report_from_history(&history(
            message("assistant", "error", Some("agent crashed")),
            vec![],
        ));
        assert_eq!(state, "failed");
        assert_eq!(report, "agent crashed");
    }

    #[test]
    fn an_empty_answer_is_a_failure_not_an_empty_report() {
        let (state, report) =
            report_from_history(&history(message("assistant", "completed", None), vec![]));
        assert_eq!(state, "failed");
        assert!(report.contains("without a final answer"));
    }

    #[test]
    fn a_history_without_an_assistant_message_fails() {
        let (state, _) = report_from_history(&history(
            message("user", "completed", None),
            vec![text_part("question")],
        ));
        assert_eq!(state, "failed");
    }

    #[test]
    fn the_last_assistant_message_wins() {
        let history = vec![
            MessageWithParts {
                message: message("assistant", "completed", None),
                parts: vec![text_part("old")],
            },
            MessageWithParts {
                message: Message {
                    id: "m2".into(),
                    ..message("assistant", "completed", None)
                },
                parts: vec![Part {
                    id: "p2".into(),
                    message_id: "m2".into(),
                    idx: 0,
                    kind: "text".into(),
                    payload_json: json!({ "text": "new" }).to_string(),
                }],
            },
        ];
        let (state, report) = report_from_history(&history);
        assert_eq!(state, "completed");
        assert_eq!(report, "new");
    }
}
