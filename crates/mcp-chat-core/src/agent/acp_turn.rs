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

use crate::acp::AcpEvent;
use crate::acp::session::{PermissionDecision, PermissionHandler, PermissionRequestInfo};
use crate::agent::ChatEngine;
use crate::agent::approvals::ApprovalDecision;
use crate::db::repo::{acp_agents, conversations, messages};
use crate::events::{CoreEvent, TurnStatus};
use crate::models::{MessageWithParts, PartPayload};
use crate::{CoreError, Result};

/// The text a parent sees when an ACP turn ends badly.
const NO_ANSWER: &str = "The ACP agent finished without a final answer.";

/// Everything a turn needs from the parent conversation.
pub struct TurnSpec<'a> {
    /// The child conversation that owns this ACP session.
    pub child_id: &'a str,
    /// The prompt text to send.
    pub prompt: &'a str,
    /// Attachments and file contexts from the same user message.
    pub attachments: Vec<crate::acp::prompt::AttachmentInput>,
    pub contexts: Vec<crate::models::MessageContext>,
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
    // Two switches, like the built-in tools: the agent's own stored policy and
    // the per-chat Auto button. Auto is the stronger of the two, so enabling it
    // now applies here instead of being silently ignored.
    let stored =
        crate::db::repo::acp_agents::PermissionPolicy::parse(&spec.agent.permission_policy)
            .unwrap_or(crate::db::repo::acp_agents::PermissionPolicy::Ask);
    let policy = if engine.auto_approve(spec.child_id) {
        crate::db::repo::acp_agents::PermissionPolicy::Auto
    } else {
        stored
    };
    let handler = permission_handler(engine.clone(), spec.child_id, &cancel, policy);

    let active = match engine.acp.take(spec.child_id).await {
        Some(active) => active,
        None => {
            // A stored id lets the agent continue the conversation after an
            // app restart; the client falls back to a fresh session when the
            // agent no longer knows it.
            let resume = conversations::get(&engine.db.pool, spec.child_id)
                .await
                .ok()
                .and_then(|conversation| conversation.acp_session_id);
            let active = engine
                .acp
                .open(crate::acp::session::SessionSpec {
                    local_id: spec.child_id.to_owned(),
                    command: &spec.agent.command,
                    args: &args,
                    env: &env,
                    cwd: spec.cwd,
                    resume,
                    permissions: handler,
                })
                .await
                .map_err(|error| CoreError::Message(error.user_message()))?;
            conversations::set_acp_session_id(
                &engine.db.pool,
                spec.child_id,
                &active.handle.acp_session_id,
            )
            .await
            .ok();
            active
        }
    };

    // Only blocks the agent advertised are sent; the builder degrades the rest
    // to a visible note rather than provoking a protocol error.
    let caps = crate::acp::prompt::PromptCapabilities {
        images: active.handle.accepts_images,
        embedded_context: active.handle.accepts_embedded_context,
    };
    let blocks =
        crate::acp::prompt::build_prompt(spec.prompt, &spec.attachments, &spec.contexts, caps);

    let events = stream_turn(
        engine.clone(),
        spec.child_id,
        blocks,
        &active.handle,
        active.events,
        cancel,
    )
    .await;
    // The stream always comes back, even after a failure: dropping the receiver
    // would close the pump's send side and leave the session unusable.
    engine.acp.put_events(spec.child_id, events).await;
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
    /// The user stopped this turn, so the transcript is incomplete on purpose.
    cancelled: bool,
}

async fn stream_turn(
    engine: Arc<ChatEngine>,
    child_id: &str,
    blocks: Vec<agent_client_protocol::v1::schema::ContentBlock>,
    handle: &Arc<crate::acp::AcpSession>,
    events: mpsc::UnboundedReceiver<AcpEvent>,
    cancel: CancellationToken,
) -> mpsc::UnboundedReceiver<AcpEvent> {
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
        cancelled: false,
    };

    if handle.prompt_blocks(blocks).is_err() {
        // Recorded on the turn so the caller still gets its stream back.
        run.failure = Some("That ACP session has ended. Start a new session.".into());
        run.finish().await;
        return run.events;
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
            // Plans, the command list, and config options are metadata: the pump
            // already stored them, and none belongs in the transcript.
            AcpEvent::Plan(_)
            | AcpEvent::PermissionRequested { .. }
            | AcpEvent::Commands(_)
            | AcpEvent::ConfigOptions(_) => {}
            AcpEvent::TurnEnded { stop_reason } => {
                match stop_reason.as_str() {
                    "end_turn" => {}
                    "cancelled" => run.cancelled = true,
                    "refusal" => {
                        run.failure = Some(
                            "The agent refused this request. Its own model declined, so nothing ran."
                                .into(),
                        )
                    }
                    "max_tokens" => {
                        run.failure = Some(
                            "The agent stopped because it reached its token limit. The answer above is incomplete."
                                .into(),
                        )
                    }
                    "max_turn_requests" => {
                        run.failure = Some(
                            "The agent stopped because it reached its limit of tool requests for one turn. Continue in a new message to let it finish."
                                .into(),
                        )
                    }
                    // An unknown reason is not a failure: the turn did end and
                    // the transcript above is what the agent produced.
                    other => {
                        tracing::debug!("ACP turn ended with an unmapped stop reason: {other}")
                    }
                }
                break;
            }
            AcpEvent::TurnFailed { message } => {
                run.failure = Some(message);
                break;
            }
        }
    }

    run.finish().await;
    run.events
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

        if !self.tools.contains_key(call_id) {
            // A tool call ends the current segment, so the report is only the
            // text that follows the last tool call.
            self.text_part = None;
        }
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
            let (status, error) = if self.cancelled {
                // The same status the built-in runner uses, so the transcript
                // renders a stop notice rather than an error.
                ("interrupted", None)
            } else {
                match &self.failure {
                    Some(message) => ("error", Some(message.clone())),
                    None => ("completed", None),
                }
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
        let notice = if self.cancelled {
            Some("Stopped.".to_owned())
        } else {
            self.failure.clone()
        };
        let _ = self.engine.events.send(CoreEvent::Status {
            conversation_id: self.child_id.to_owned(),
            status: TurnStatus::Idle,
            message: notice,
        });
    }
}

/// Whether the agent's configured policy settles this request without asking.
///
/// Only read-only work can be auto-approved. Everything else, including an
/// action whose kind the agent did not report, goes to the user.
fn policy_allows(
    policy: crate::db::repo::acp_agents::PermissionPolicy,
    kind: Option<&str>,
) -> bool {
    let kind = crate::db::repo::acp_agents::PermissionKind::parse(kind);
    crate::db::repo::acp_agents::auto_decide(policy, kind)
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
    policy: crate::db::repo::acp_agents::PermissionPolicy,
) -> PermissionHandler {
    let cancel = cancel.clone();
    let child_id = child_id.to_owned();
    Arc::new(move |info: PermissionRequestInfo| {
        let engine = engine.clone();
        let child_id = child_id.clone();
        let cancel = cancel.clone();
        Box::pin(async move {
            if policy_allows(policy, info.kind.as_deref()) {
                return PermissionDecision::Approve;
            }
            let call_id = format!("acp-{}", uuid::Uuid::new_v4());
            // Register before announcing: the UI may answer the moment it sees
            // the event, and an answer for an unknown call id would be lost.
            let receiver = engine.approvals.register(&call_id);
            // ACP gives whole files; join them so the existing modal shows
            // everything the agent intends to change.
            let diff = (!info.diffs.is_empty()).then(|| info.diffs.join("\n"));
            let mut args = json!({
                "kind": info.kind,
                "options": info
                    .options
                    .iter()
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>(),
            });
            if !info.locations.is_empty() {
                args["locations"] = json!(info.locations);
            }
            let _ = engine.events.send(CoreEvent::Approval {
                conversation_id: child_id.clone(),
                call_id: call_id.clone(),
                server: "ACP".into(),
                tool: info.title.clone(),
                args,
                // The agent remembers "always" itself, and ACP has no way for
                // us to scope a rule it owns.
                allow_always: false,
                action: info.kind.clone().unwrap_or_else(|| "tool".into()),
                resource: info
                    .locations
                    .first()
                    .cloned()
                    .unwrap_or_else(|| info.title.clone()),
                save_pattern: None,
                diff,
            });
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
                // A rejection's feedback is dropped on purpose: ACP's outcome is
                // just `reject_once`, with no field to carry text back. The
                // modal hides its comment box for ACP for the same reason.
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
    if last.message.status == "interrupted" {
        // The user stopped it, so the answer is incomplete by definition. A
        // parent that is told "completed" here would build on half an answer.
        return (
            "failed".to_owned(),
            "The ACP turn was stopped before it finished, so its answer is incomplete.".to_owned(),
        );
    }
    // An ACP turn starts a new text part after each tool call, so the last
    // non-empty one is the report: the narration before a tool call is not the
    // answer the parent asked for.
    let report = last
        .parts
        .iter()
        .rev()
        .filter(|part| part.kind == "text")
        .filter_map(|part| {
            part.payload()
                .get("text")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .find(|text| !text.trim().is_empty());
    match report {
        Some(report) => ("completed".to_owned(), report.trim().to_owned()),
        None => ("failed".to_owned(), NO_ANSWER.to_owned()),
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
    fn auto_reads_policy_settles_read_only_requests() {
        use crate::db::repo::acp_agents::PermissionPolicy;
        // Reads and searches never reach the user under this policy.
        assert!(policy_allows(PermissionPolicy::AutoReads, Some("read")));
        assert!(policy_allows(PermissionPolicy::AutoReads, Some("search")));
        assert!(policy_allows(PermissionPolicy::AutoReads, Some("think")));
        // Anything that can change state still asks.
        assert!(!policy_allows(PermissionPolicy::AutoReads, Some("edit")));
        assert!(!policy_allows(PermissionPolicy::AutoReads, Some("execute")));
        assert!(!policy_allows(PermissionPolicy::AutoReads, Some("delete")));
        // An unreported kind must not be guessed as harmless.
        assert!(!policy_allows(PermissionPolicy::AutoReads, None));
        assert!(!policy_allows(PermissionPolicy::AutoReads, Some("mystery")));
    }

    #[test]
    fn an_interrupted_message_is_not_a_completed_report() {
        let (state, report) = report_from_history(&history(
            message("assistant", "interrupted", None),
            vec![text_part("half an answer")],
        ));
        assert_eq!(state, "failed");
        assert!(report.contains("stopped"), "{report}");
    }

    #[test]
    fn auto_policy_allows_everything_except_delete() {
        use crate::db::repo::acp_agents::PermissionPolicy;
        assert!(policy_allows(PermissionPolicy::Auto, Some("read")));
        assert!(policy_allows(PermissionPolicy::Auto, Some("edit")));
        assert!(policy_allows(PermissionPolicy::Auto, Some("execute")));
        assert!(policy_allows(PermissionPolicy::Auto, Some("move")));
        // Irreversible, so it still asks even in Auto.
        assert!(!policy_allows(PermissionPolicy::Auto, Some("delete")));
        // An unreported kind is allowed here: the user asked for automatic
        // approval, and refusing unknown kinds would make the setting look
        // broken as soon as an agent adds a tool.
        assert!(policy_allows(PermissionPolicy::Auto, None));
        assert!(policy_allows(PermissionPolicy::Auto, Some("mystery")));
    }

    #[test]
    fn auto_reads_still_refuses_edits_even_though_auto_allows_them() {
        use crate::db::repo::acp_agents::PermissionPolicy;
        assert!(!policy_allows(PermissionPolicy::AutoReads, Some("edit")));
        assert!(policy_allows(PermissionPolicy::Auto, Some("edit")));
    }

    #[test]
    fn ask_policy_never_settles_anything() {
        use crate::db::repo::acp_agents::PermissionPolicy;
        for kind in [Some("read"), Some("search"), Some("think"), None] {
            assert!(!policy_allows(PermissionPolicy::Ask, kind), "{kind:?}");
        }
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
