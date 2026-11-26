//! One persistent ACP session: process, connection, and turn pump.
//!
//! The connection is driven by a single task that owns the SDK `ActiveSession`.
//! Prompts arrive over a channel and updates leave over a channel. Cancelling
//! uses a cloned connection handle, so the pump never needs to share
//! `&mut ActiveSession` between a read and a command.

use std::path::Path;
use std::sync::Arc;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    CancelNotification, CloseSessionRequest, InitializeRequest, RequestPermissionRequest,
    SessionNotification, SessionUpdate,
};
use agent_client_protocol::v1::{ConnectionTo, SessionMessage, SessionSetup};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectTo};
use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::error::AcpError;
use super::events::AcpEvent;
use super::process::AgentProcess;

/// How to answer one permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Approve,
    Reject,
    Cancel,
}

/// A permission request, already reduced to what a decision needs.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequestInfo {
    pub title: String,
    pub kind: Option<String>,
    /// `(option_id, kind)` as offered by the agent.
    pub options: Vec<(String, Option<String>)>,
}

pub type PermissionHandler =
    Arc<dyn Fn(PermissionRequestInfo) -> BoxFuture<'static, PermissionDecision> + Send + Sync>;

/// Commands accepted by a running session.
enum Command {
    Prompt(String),
    Close,
}

/// Handle to one live session.
pub struct AcpSession {
    pub local_id: String,
    pub acp_session_id: String,
    pub command: String,
    /// Whether the agent advertised `sessionCapabilities.close`. Most do not,
    /// and sending the request anyway would report a method-not-found error.
    pub supports_close: bool,
    /// Cleared by the pump when the process or connection ends, so the manager
    /// can drop a dead session instead of leaking one of its slots.
    alive: Arc<std::sync::atomic::AtomicBool>,
    prompts: mpsc::UnboundedSender<Command>,
    connection: ConnectionTo<Agent>,
    acp_id: String,
}

impl AcpSession {
    /// Queue a prompt. Events for the turn are emitted on the session sink.
    pub fn prompt(&self, text: impl Into<String>) -> Result<(), AcpError> {
        self.prompts
            .send(Command::Prompt(text.into()))
            .map_err(|_| AcpError::Inactive)
    }

    /// Ask the agent to stop the current turn. The session stays usable.
    pub fn cancel(&self) -> Result<(), AcpError> {
        self.connection
            .send_notification_to(Agent, CancelNotification::new(self.acp_id.clone()))
            .map_err(|error| AcpError::Protocol {
                message: error.to_string(),
            })
    }

    /// Whether the pump is still running.
    pub fn is_alive(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Close the session and let the pump terminate the process.
    ///
    /// The agent is asked to close only when it advertised support; either way
    /// the pump stops and the child process is torn down.
    pub fn close(&self) -> Result<(), AcpError> {
        // A closed channel means the pump is already gone, so the session is
        // already closed; that is success, not a failure to report.
        let _ = self.prompts.send(Command::Close);
        Ok(())
    }
}

/// Everything needed to start one session.
pub struct SessionSpec<'a> {
    pub local_id: String,
    pub command: &'a str,
    pub args: &'a [String],
    pub env: &'a [(String, String)],
    /// Absolute working directory for the child and for `session/new`.
    pub cwd: &'a Path,
    pub permissions: PermissionHandler,
}

/// Spawn the agent, initialize, create a session, and leave the pump running.
///
/// `events` is the sink for turn updates. The caller keeps the matching
/// receiver, so the pump can always report why it stopped.
pub async fn open(
    spec: SessionSpec<'_>,
    events: mpsc::UnboundedSender<AcpEvent>,
) -> Result<AcpSession, AcpError> {
    let mut process = AgentProcess::spawn(spec.command, spec.args, spec.env, Some(spec.cwd))?;
    let stdout = process
        .child
        .stdout
        .take()
        .ok_or_else(|| AcpError::SpawnFailed {
            command: spec.command.to_owned(),
            message: "the agent exposed no stdout".into(),
        })?;
    let stdin = process
        .child
        .stdin
        .take()
        .ok_or_else(|| AcpError::SpawnFailed {
            command: spec.command.to_owned(),
            message: "the agent exposed no stdin".into(),
        })?;
    let stderr = process.child.stderr.take();
    let stderr_tail =
        std::sync::Arc::new(std::sync::Mutex::new(super::process::StderrTail::default()));

    if let Some(stderr) = stderr {
        let label = spec.local_id.clone();
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            // Draining stderr matters: a full pipe would stall the agent, and
            // the tail is what explains a premature exit.
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(agent = %label, "ACP agent stderr: {line}");
                if let Ok(mut tail) = tail.lock() {
                    tail.push(line);
                }
            }
        });
    }

    let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel::<Result<SessionReady, AcpError>>();
    let permissions = spec.permissions.clone();
    let cwd = spec.cwd.to_path_buf();

    let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let pump_alive = alive.clone();
    let pump_tail = stderr_tail.clone();
    let pump = tokio::spawn(async move {
        let result = drive(
            transport,
            prompt_rx,
            events.clone(),
            permissions,
            cwd,
            ready_tx,
        )
        .await;
        // A turn failure has already been reported. Only a transport-level
        // failure is left to explain here, and the stderr tail is the most
        // useful thing to say.
        if let Err(error) = result {
            let tail = pump_tail
                .lock()
                .map(|tail| tail.to_string())
                .unwrap_or_default();
            let message = if tail.trim().is_empty() {
                error.user_message()
            } else {
                format!("{}\n{}", error.user_message(), tail)
            };
            let _ = events.send(AcpEvent::TurnFailed { message });
        }
        pump_alive.store(false, std::sync::atomic::Ordering::SeqCst);
        process.terminate().await;
    });

    let ready = match ready_rx.await {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            pump.abort();
            return Err(error);
        }
        Err(_) => {
            pump.abort();
            return Err(AcpError::Crashed {
                stderr_tail: String::new(),
            });
        }
    };

    Ok(AcpSession {
        local_id: spec.local_id,
        acp_session_id: ready.session_id.clone(),
        command: spec.command.to_owned(),
        supports_close: ready.supports_close,
        alive,
        prompts: prompt_tx,
        connection: ready.connection,
        acp_id: ready.session_id,
    })
}

/// What the caller needs once the connection is live.
struct SessionReady {
    connection: ConnectionTo<Agent>,
    session_id: String,
    supports_close: bool,
}

async fn drive<T>(
    transport: T,
    mut prompts: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<AcpEvent>,
    permissions: PermissionHandler,
    cwd: std::path::PathBuf,
    ready: oneshot::Sender<Result<SessionReady, AcpError>>,
) -> Result<(), AcpError>
where
    T: ConnectTo<Client> + Send + 'static,
{
    let outcome = Client
        .v1()
        .name("jarvis")
        .connect_with(transport, async move |cx| {
            let initialized = cx
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await;
            let initialized = match initialized {
                Ok(response) => response,
                Err(error) => {
                    let _ = ready.send(Err(map_sdk_error(error)));
                    return Ok(());
                }
            };
            let supports_close = initialized
                .agent_capabilities
                .session_capabilities
                .close
                .is_some();

            // ACP requires an absolute cwd; callers canonicalize it.
            // `start_session` returns a `'static` session, so the pump can loop
            // here without borrowing the closure's stack frame.
            let setup = cx.build_session(&cwd).block_task().start_session().await;
            let SessionSetup { mut session, .. } = match setup {
                Ok(setup) => setup,
                Err(error) => {
                    let _ = ready.send(Err(map_sdk_error(error)));
                    return Ok(());
                }
            };

            let _ = ready.send(Ok(SessionReady {
                connection: session.connection().clone(),
                session_id: session.session_id().to_string(),
                supports_close,
            }));

            // Every turn failure is reported exactly once, right where it
            // becomes known; the pump wrapper only reports transport failures.
            'turn: while let Some(command) = prompts.recv().await {
                match command {
                    Command::Close => {
                        // `session/close` is opt-in: only call it when the agent
                        // advertised the capability, or it answers with
                        // method-not-found.
                        if supports_close {
                            let _ = session
                                .connection()
                                .send_request(CloseSessionRequest::new(
                                    session.session_id().clone(),
                                ))
                                .block_task()
                                .await;
                        }
                        break;
                    }
                    Command::Prompt(text) => {
                        if session.send_prompt(text).is_err() {
                            let _ = events.send(AcpEvent::TurnFailed {
                                message: AcpError::Inactive.user_message(),
                            });
                            break 'turn;
                        }
                        // Only this turn ends on a failed prompt: the agent may
                        // still be healthy and accept the next one.
                        loop {
                            match session.read_update().await {
                                Ok(SessionMessage::PromptResponse(Ok(response))) => {
                                    let _ = events.send(AcpEvent::TurnEnded {
                                        stop_reason: stop_reason(&response),
                                    });
                                    break;
                                }
                                Ok(SessionMessage::PromptResponse(Err(error))) => {
                                    let _ = events.send(AcpEvent::TurnFailed {
                                        message: map_sdk_error(error).user_message(),
                                    });
                                    break;
                                }
                                Ok(SessionMessage::Message(dispatch)) => {
                                    handle_dispatch(dispatch.into_raw(), &events, &permissions)
                                        .await;
                                }
                                // `SessionMessage` is non_exhaustive: ignore
                                // variants this SDK version does not define.
                                Ok(_) => {}
                                Err(error) => {
                                    // The update stream is gone, so the
                                    // connection is dead and no turn can follow.
                                    let _ = events.send(AcpEvent::TurnFailed {
                                        message: map_sdk_error(error).user_message(),
                                    });
                                    break 'turn;
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        })
        .await;

    outcome.map_err(map_sdk_error)
}

async fn handle_dispatch(
    dispatch: agent_client_protocol::Dispatch,
    events: &mpsc::UnboundedSender<AcpEvent>,
    permissions: &PermissionHandler,
) {
    let events_for_update = events.clone();
    let permissions = permissions.clone();
    let result = agent_client_protocol::util::MatchDispatch::new(dispatch)
        .if_notification(|notification: SessionNotification| async move {
            emit_update(notification.update, &events_for_update);
            Ok(())
        })
        .await
        .if_request(
            |request: RequestPermissionRequest,
             responder: agent_client_protocol::Responder<
                agent_client_protocol::v1::schema::RequestPermissionResponse,
            >| async move {
                let info = permission_info(&request);
                let decision = permissions(info.clone()).await;
                let _ = events.send(AcpEvent::PermissionRequested {
                    request_id: info.title.clone(),
                    title: info.title.clone(),
                    kind: info.kind.clone(),
                    options: info.options.iter().map(|(id, _)| id.clone()).collect(),
                });
                responder.respond(permission_response(&info, decision))
            },
        )
        .await
        .otherwise_ignore();
    if let Err(error) = result {
        tracing::debug!("ACP dispatch was not handled: {error}");
    }
}

fn emit_update(update: SessionUpdate, events: &mpsc::UnboundedSender<AcpEvent>) {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let Some(text) = content_text(&chunk.content) {
                let _ = events.send(AcpEvent::Text(text));
            }
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            if let Some(text) = content_text(&chunk.content) {
                let _ = events.send(AcpEvent::Thought(text));
            }
        }
        SessionUpdate::ToolCall(call) => {
            let _ = events.send(AcpEvent::ToolCall {
                call_id: call.tool_call_id.to_string(),
                title: call.title,
                kind: Some(wire_name(&call.kind)),
                status: wire_name(&call.status),
                detail: None,
            });
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let fields = update.fields;
            let _ = events.send(AcpEvent::ToolCall {
                call_id: update.tool_call_id.to_string(),
                title: fields.title.unwrap_or_default(),
                kind: fields.kind.map(|kind| wire_name(&kind)),
                status: fields
                    .status
                    .map(|status| wire_name(&status))
                    .unwrap_or_else(|| "in_progress".to_owned()),
                detail: None,
            });
        }
        SessionUpdate::Plan(plan) => {
            let _ = events.send(AcpEvent::Plan(format!(
                "{} plan entries",
                plan.entries.len()
            )));
        }
        _ => {}
    }
}

/// The protocol's wire name for an enum value (`AllowOnce` -> `allow_once`).
///
/// `Debug` yields `AllowOnce`, which would never match the snake_case names
/// used by `select_option` and the rest of the protocol.
fn wire_name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn content_text(content: &agent_client_protocol::v1::schema::ContentBlock) -> Option<String> {
    match content {
        agent_client_protocol::v1::schema::ContentBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    }
}

fn permission_info(request: &RequestPermissionRequest) -> PermissionRequestInfo {
    PermissionRequestInfo {
        title: request
            .tool_call
            .fields
            .title
            .clone()
            .unwrap_or_else(|| "ACP tool call".to_owned()),
        kind: request.tool_call.fields.kind.map(|kind| wire_name(&kind)),
        options: request
            .options
            .iter()
            .map(|option| (option.option_id.to_string(), Some(wire_name(&option.kind))))
            .collect(),
    }
}

fn permission_response(
    info: &PermissionRequestInfo,
    decision: PermissionDecision,
) -> agent_client_protocol::v1::schema::RequestPermissionResponse {
    use agent_client_protocol::v1::schema::{
        RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome,
    };

    let options = info
        .options
        .iter()
        .map(|(id, kind)| super::PermissionOption {
            option_id: id.clone(),
            kind: kind.clone().unwrap_or_default(),
        })
        .collect::<Vec<_>>();

    match super::select_option(&options, decision == PermissionDecision::Approve) {
        super::PermissionOutcome::Select(id) => RequestPermissionResponse::new(
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
        ),
        super::PermissionOutcome::Cancel => {
            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
        }
    }
}

fn stop_reason(response: &agent_client_protocol::v1::schema::PromptResponse) -> String {
    wire_name(&response.stop_reason)
}

fn map_sdk_error(error: agent_client_protocol::Error) -> AcpError {
    let message = error.to_string();
    let lower = message.to_lowercase();
    if lower.contains("auth") || lower.contains("login") || lower.contains("unauthorized") {
        return AcpError::AuthRequired {
            agent: "ACP agent".to_owned(),
        };
    }
    if agent_client_protocol::is_incoming_transport_closed(&error)
        || agent_client_protocol::is_connection_terminated(&error)
    {
        return AcpError::Crashed {
            stderr_tail: String::new(),
        };
    }
    AcpError::Protocol { message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v1::schema::{PermissionOptionKind, ToolCallStatus, ToolKind};

    #[test]
    fn permission_kinds_use_their_wire_names() {
        // `Debug` would give `AllowOnce`, which never matches `select_option`.
        assert_eq!(wire_name(&PermissionOptionKind::AllowOnce), "allow_once");
        assert_eq!(
            wire_name(&PermissionOptionKind::AllowAlways),
            "allow_always"
        );
        assert_eq!(wire_name(&PermissionOptionKind::RejectOnce), "reject_once");
        assert_eq!(
            wire_name(&PermissionOptionKind::RejectAlways),
            "reject_always"
        );
    }

    #[test]
    fn tool_names_and_statuses_use_their_wire_names() {
        assert_eq!(wire_name(&ToolKind::Read), "read");
        assert_eq!(wire_name(&ToolKind::Execute), "execute");
        assert_eq!(wire_name(&ToolCallStatus::Pending), "pending");
        assert_eq!(wire_name(&ToolCallStatus::InProgress), "in_progress");
        assert_eq!(wire_name(&ToolCallStatus::Completed), "completed");
        assert_eq!(wire_name(&ToolCallStatus::Failed), "failed");
    }

    #[test]
    fn wire_names_make_approval_pick_allow_once() {
        let info = PermissionRequestInfo {
            title: "Run tests".into(),
            kind: Some(wire_name(&ToolKind::Execute)),
            options: [
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
                PermissionOptionKind::RejectOnce,
            ]
            .into_iter()
            .map(|kind| (wire_name(&kind), Some(wire_name(&kind))))
            .collect(),
        };
        let response = permission_response(&info, PermissionDecision::Approve);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["outcome"]["outcome"], "selected");
        assert_eq!(json["outcome"]["optionId"], "allow_once");
    }

    #[test]
    fn an_allow_always_only_request_is_cancelled() {
        let info = PermissionRequestInfo {
            title: "Run tests".into(),
            kind: Some(wire_name(&ToolKind::Execute)),
            options: [PermissionOptionKind::AllowAlways]
                .into_iter()
                .map(|kind| (wire_name(&kind), Some(wire_name(&kind))))
                .collect(),
        };
        let response = permission_response(&info, PermissionDecision::Approve);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn rejection_prefers_reject_once() {
        let info = PermissionRequestInfo {
            title: "Run tests".into(),
            kind: Some(wire_name(&ToolKind::Execute)),
            options: [
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways,
            ]
            .into_iter()
            .map(|kind| (wire_name(&kind), Some(wire_name(&kind))))
            .collect(),
        };
        let response = permission_response(&info, PermissionDecision::Reject);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["outcome"]["optionId"], "reject_once");
    }
}
