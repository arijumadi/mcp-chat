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

    /// Close the session and let the pump terminate the process.
    pub fn close(&self) -> Result<(), AcpError> {
        self.prompts
            .send(Command::Close)
            .map_err(|_| AcpError::Inactive)
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
    let (ready_tx, ready_rx) = oneshot::channel();
    let permissions = spec.permissions.clone();
    let cwd = spec.cwd.to_path_buf();

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
        // When the pump ends while the UI still expects a turn, say why instead
        // of leaving the channel to close silently.
        let tail = pump_tail
            .lock()
            .map(|tail| tail.to_string())
            .unwrap_or_default();
        let message = match result {
            Err(error) => error.user_message(),
            Ok(()) => {
                if tail.trim().is_empty() {
                    "The ACP session ended unexpectedly.".to_owned()
                } else {
                    format!("The ACP session ended unexpectedly:\n{tail}")
                }
            }
        };
        let _ = events.send(AcpEvent::TurnFailed { message });
        process.terminate().await;
    });

    let (connection, acp_session_id) = match ready_rx.await {
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
        acp_session_id: acp_session_id.clone(),
        command: spec.command.to_owned(),
        prompts: prompt_tx,
        connection,
        acp_id: acp_session_id,
    })
}

async fn drive<T>(
    transport: T,
    mut prompts: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<AcpEvent>,
    permissions: PermissionHandler,
    cwd: std::path::PathBuf,
    ready: oneshot::Sender<Result<(ConnectionTo<Agent>, String), AcpError>>,
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
            if let Err(error) = initialized {
                let _ = ready.send(Err(map_sdk_error(error)));
                return Ok(());
            }

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

            let _ = ready.send(Ok((
                session.connection().clone(),
                session.session_id().to_string(),
            )));

            while let Some(command) = prompts.recv().await {
                match command {
                    Command::Close => {
                        let _ = session
                            .connection()
                            .send_request(CloseSessionRequest::new(session.session_id().clone()))
                            .block_task()
                            .await;
                        break;
                    }
                    Command::Prompt(text) => {
                        if session.send_prompt(text).is_err() {
                            let _ = events.send(AcpEvent::TurnFailed {
                                message: AcpError::Inactive.user_message(),
                            });
                            break;
                        }
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
                                    let _ = events.send(AcpEvent::TurnFailed {
                                        message: map_sdk_error(error).user_message(),
                                    });
                                    break;
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
                kind: Some(format!("{:?}", call.kind).to_lowercase()),
                status: format!("{:?}", call.status).to_lowercase(),
                detail: None,
            });
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let fields = update.fields;
            let _ = events.send(AcpEvent::ToolCall {
                call_id: update.tool_call_id.to_string(),
                title: fields.title.unwrap_or_default(),
                kind: fields.kind.map(|kind| format!("{kind:?}").to_lowercase()),
                status: fields
                    .status
                    .map(|status| format!("{status:?}").to_lowercase())
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
        kind: request
            .tool_call
            .fields
            .kind
            .map(|kind| format!("{kind:?}").to_lowercase()),
        options: request
            .options
            .iter()
            .map(|option| {
                let kind = format!("{:?}", option.kind).to_lowercase();
                (option.option_id.to_string(), Some(kind))
            })
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
    serde_json::to_value(response.stop_reason)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "end_turn".to_owned())
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
