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
use agent_client_protocol::v1::{ConnectionTo, SessionMessage};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectTo};
use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::error::AcpError;
use super::events::{AcpEvent, AgentCommand};
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
    /// File changes the agent included with the request, already rendered as
    /// unified diffs. An empty vector means the request showed no diff.
    pub diffs: Vec<String>,
    /// Absolute paths the tool call touches, when the agent reported any.
    pub locations: Vec<String>,
}

/// The agent's slash commands, shared between the pump and the handle.
type CommandStore = Arc<std::sync::Mutex<Vec<AgentCommand>>>;

pub type PermissionHandler =
    Arc<dyn Fn(PermissionRequestInfo) -> BoxFuture<'static, PermissionDecision> + Send + Sync>;

/// Commands accepted by a running session.
enum Command {
    Prompt(Vec<agent_client_protocol::v1::schema::ContentBlock>),
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
    /// Name and version the agent reported during `initialize`, if any.
    pub agent_info: Option<(String, String)>,
    /// What the agent accepts beyond plain text. Sending an image block to an
    /// agent without `image` is a protocol error, so the prompt builder must
    /// know this before it assembles a turn.
    pub accepts_images: bool,
    pub accepts_embedded_context: bool,
    /// Slash commands the agent advertised, replaced whenever it announces a
    /// new set. Kept on the handle so the composer can ask at any time without
    /// needing a turn in flight.
    commands: CommandStore,
    /// Cleared by the pump when the process or connection ends, so the manager
    /// can drop a dead session instead of leaking one of its slots.
    alive: Arc<std::sync::atomic::AtomicBool>,
    resumed: bool,
    prompts: mpsc::UnboundedSender<Command>,
    connection: ConnectionTo<Agent>,
    acp_id: String,
}

impl AcpSession {
    /// Queue a text-only prompt.
    pub fn prompt(&self, text: impl Into<String>) -> Result<(), AcpError> {
        self.prompt_blocks(vec![agent_client_protocol::v1::schema::ContentBlock::Text(
            agent_client_protocol::v1::schema::TextContent::new(text.into()),
        )])
    }

    /// Queue a prompt made of content blocks.
    ///
    /// The caller is responsible for using only blocks the agent advertised
    /// during `initialize`; this method does not second-guess it.
    pub fn prompt_blocks(
        &self,
        blocks: Vec<agent_client_protocol::v1::schema::ContentBlock>,
    ) -> Result<(), AcpError> {
        self.prompts
            .send(Command::Prompt(blocks))
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

    /// The agent's current slash commands.
    pub fn commands(&self) -> Vec<AgentCommand> {
        self.commands
            .lock()
            .map(|commands| commands.clone())
            .unwrap_or_default()
    }

    /// Whether this handle continues a session the agent already knew.
    pub fn resumed(&self) -> bool {
        self.resumed
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
    /// Absolute working directory for the child and for the session request.
    pub cwd: &'a Path,
    /// Resume an existing ACP session instead of creating a new one. Only
    /// honoured when the agent advertises `sessionCapabilities.resume`.
    pub resume: Option<String>,
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
    let resume = spec.resume.clone();

    let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let pump_alive = alive.clone();
    let commands: CommandStore = Arc::new(std::sync::Mutex::new(Vec::new()));
    let pump_commands = commands.clone();
    let pump_tail = stderr_tail.clone();
    let pump = tokio::spawn(async move {
        let result = drive(
            transport,
            Pump {
                prompts: prompt_rx,
                events: events.clone(),
                permissions,
                cwd,
                resume,
                commands: pump_commands,
                ready: ready_tx,
            },
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
        agent_info: ready.agent_info,
        commands,
        accepts_images: ready.accepts_images,
        accepts_embedded_context: ready.accepts_embedded_context,
        alive,
        resumed: ready.resumed,
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
    /// True when the agent accepted the requested resume id, false when a new
    /// session was created (including after a failed resume).
    resumed: bool,
    agent_info: Option<(String, String)>,
    accepts_images: bool,
    accepts_embedded_context: bool,
}

/// Everything the connection task owns for its lifetime.
struct Pump {
    prompts: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<AcpEvent>,
    permissions: PermissionHandler,
    cwd: std::path::PathBuf,
    resume: Option<String>,
    commands: CommandStore,
    ready: oneshot::Sender<Result<SessionReady, AcpError>>,
}

async fn drive<T>(transport: T, pump: Pump) -> Result<(), AcpError>
where
    T: ConnectTo<Client> + Send + 'static,
{
    let Pump {
        mut prompts,
        events,
        permissions,
        cwd,
        resume,
        commands,
        ready,
    } = pump;
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
                    let _ = ready.send(Err(map_error(error, &AgentFacts::default())));
                    return Ok(());
                }
            };
            // Everything after this point can name the agent and its login
            // methods in an error, which is what the settings Probe shows.
            let facts = AgentFacts::from_init(&initialized);
            let session_caps = &initialized.agent_capabilities.session_capabilities;
            let supports_close = session_caps.close.is_some();
            let supports_resume = session_caps.resume.is_some();
            let agent_info = initialized
                .agent_info
                .as_ref()
                .map(|info| (info.name.clone(), info.version.clone()));
            let prompt_caps = &initialized.agent_capabilities.prompt_capabilities;
            let accepts_images = prompt_caps.image;
            let accepts_embedded_context = prompt_caps.embedded_context;

            // ACP requires an absolute cwd; callers canonicalize it.
            // `start_session` returns a `'static` session, so the pump can loop
            // here without borrowing the closure's stack frame.
            //
            // Resume is best effort: a stale id (the agent deleted the session,
            // or the app was restarted against a different agent state) must
            // fall back to a fresh session rather than fail the turn.
            let mut resumed = false;
            let new_session = || async {
                cx.build_session(&cwd)
                    .block_task()
                    .start_session()
                    .await
                    .map(|setup| setup.session)
            };
            let mut session = match (resume, supports_resume) {
                (Some(session_id), true) => {
                    match cx
                        .resume_session(session_id, &cwd)
                        .block_task()
                        .start_session()
                        .await
                    {
                        Ok(setup) => {
                            resumed = true;
                            setup.session
                        }
                        Err(error) => {
                            tracing::debug!(
                                "ACP session resume failed, starting a new session: {error}"
                            );
                            match new_session().await {
                                Ok(session) => session,
                                Err(error) => {
                                    let _ = ready.send(Err(map_error(error, &facts)));
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
                _ => match new_session().await {
                    Ok(session) => session,
                    Err(error) => {
                        let _ = ready.send(Err(map_error(error, &facts)));
                        return Ok(());
                    }
                },
            };

            let _ = ready.send(Ok(SessionReady {
                connection: session.connection().clone(),
                session_id: session.session_id().to_string(),
                supports_close,
                resumed,
                agent_info,
                accepts_images,
                accepts_embedded_context,
            }));

            // Every turn failure is reported exactly once, right where it
            // becomes known; the pump wrapper only reports transport failures.
            //
            // Reading updates while idle matters: agents announce their slash
            // commands right after `session/new`, before any prompt, and an
            // announcement parked in the channel until the first turn would
            // make the composer look empty.
            'turn: loop {
                let next = tokio::select! {
                    command = prompts.recv() => match command {
                        Some(command) => Next::Command(command),
                        // Every handle was dropped.
                        None => break 'turn,
                    },
                    update = session.read_update() => Next::Update(Box::new(update)),
                };

                match next {
                    Next::Update(result) => match *result {
                        Ok(SessionMessage::Message(dispatch)) => {
                            handle_dispatch(dispatch.into_raw(), &events, &permissions, &commands)
                                .await;
                        }
                        // A prompt response with no turn in flight is stale;
                        // there is nothing to settle.
                        Ok(_) => {}
                        Err(error) => {
                            let _ = events.send(AcpEvent::TurnFailed {
                                message: map_error(error, &facts).user_message(),
                            });
                            break 'turn;
                        }
                    },
                    Next::Command(Command::Close) => {
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
                        break 'turn;
                    }
                    Next::Command(Command::Prompt(blocks)) => {
                        if session.send_prompt_blocks(blocks).is_err() {
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
                                    let fatal = connection_is_gone(&error);
                                    let message = map_error(error, &facts).user_message();
                                    let _ = events.send(AcpEvent::TurnFailed { message });
                                    // A failed prompt is usually recoverable,
                                    // but a closed transport is not: waiting
                                    // for another prompt would keep the slot
                                    // reserved forever.
                                    if fatal {
                                        break 'turn;
                                    }
                                    break;
                                }
                                Ok(SessionMessage::Message(dispatch)) => {
                                    handle_dispatch(
                                        dispatch.into_raw(),
                                        &events,
                                        &permissions,
                                        &commands,
                                    )
                                    .await;
                                }
                                // `SessionMessage` is non_exhaustive: ignore
                                // variants this SDK version does not define.
                                Ok(_) => {}
                                Err(error) => {
                                    // The update stream is gone, so the
                                    // connection is dead and no turn can follow.
                                    let _ = events.send(AcpEvent::TurnFailed {
                                        message: map_error(error, &facts).user_message(),
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

    // A failure here is transport-level: the per-turn errors were already
    // reported on the event stream, so the facts learned at init are gone.
    outcome.map_err(|error| map_error(error, &AgentFacts::default()))
}

/// What the idle pump can wake up for.
enum Next {
    Command(Command),
    // Boxed: a `Result<SessionMessage, _>` is far larger than a `Command`, and
    // the pump wakes up for one or the other thousands of times a turn.
    Update(Box<Result<SessionMessage, agent_client_protocol::Error>>),
}

async fn handle_dispatch(
    dispatch: agent_client_protocol::Dispatch,
    events: &mpsc::UnboundedSender<AcpEvent>,
    permissions: &PermissionHandler,
    commands: &CommandStore,
) {
    let events_for_update = events.clone();
    let commands_for_update = commands.clone();
    let permissions = permissions.clone();
    let result = agent_client_protocol::util::MatchDispatch::new(dispatch)
        .if_notification(|notification: SessionNotification| async move {
            emit_update(
                notification.update,
                &events_for_update,
                &commands_for_update,
            );
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

fn emit_update(
    update: SessionUpdate,
    events: &mpsc::UnboundedSender<AcpEvent>,
    commands: &CommandStore,
) {
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
        SessionUpdate::AvailableCommandsUpdate(update) => {
            // The protocol replaces the whole set, so store and emit the same
            // list instead of merging.
            let next: Vec<AgentCommand> = update
                .available_commands
                .into_iter()
                .map(|command| AgentCommand {
                    name: command.name,
                    description: command.description,
                    input_hint: command.input.and_then(|input| match input {
                        agent_client_protocol::v1::schema::AvailableCommandInput::Unstructured(
                            unstructured,
                        ) => Some(unstructured.hint).filter(|hint| !hint.trim().is_empty()),
                        // The enum is non_exhaustive; an unknown input shape is
                        // better shown as "takes arguments" than dropped.
                        _ => Some("arguments".to_owned()),
                    }),
                })
                .collect();
            if let Ok(mut stored) = commands.lock() {
                *stored = next.clone();
            }
            let _ = events.send(AcpEvent::Commands(next));
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

/// Render the diff content the agent attached to a permission request.
///
/// ACP sends whole files, not patches, so the app builds the unified diff with
/// the same renderer its own file tools use.
fn permission_diffs(request: &RequestPermissionRequest) -> Vec<String> {
    let mut rendered = Vec::new();
    let Some(content) = request.tool_call.fields.content.as_ref() else {
        return rendered;
    };
    for item in content {
        if let agent_client_protocol::v1::schema::ToolCallContent::Diff(diff) = item {
            let path = diff.path.display().to_string();
            let before = diff.old_text.as_deref().unwrap_or_default();
            let text = crate::agent::diff::file_diff(&path, before, &diff.new_text);
            if !text.is_empty() {
                rendered.push(crate::agent::diff::cap(text));
            }
        }
    }
    rendered
}

fn permission_locations(request: &RequestPermissionRequest) -> Vec<String> {
    request
        .tool_call
        .fields
        .locations
        .as_ref()
        .map(|locations| {
            locations
                .iter()
                .map(|location| location.path.display().to_string())
                .collect()
        })
        .unwrap_or_default()
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
        diffs: permission_diffs(request),
        locations: permission_locations(request),
    }
}

fn permission_response(
    info: &PermissionRequestInfo,
    decision: PermissionDecision,
) -> agent_client_protocol::v1::schema::RequestPermissionResponse {
    use agent_client_protocol::v1::schema::{
        RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome,
    };

    // Cancelling a turn is not a rejection: the protocol requires the
    // `cancelled` outcome so the agent stops instead of remembering a refusal.
    if decision == PermissionDecision::Cancel {
        return RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
    }

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

fn connection_is_gone(error: &agent_client_protocol::Error) -> bool {
    agent_client_protocol::is_incoming_transport_closed(error)
        || agent_client_protocol::is_connection_terminated(error)
}

/// What the mapping needs beyond the raw error, learned during `initialize`.
#[derive(Debug, Clone, Default)]
struct AgentFacts {
    label: String,
    auth_methods: Vec<String>,
}

impl AgentFacts {
    fn from_init(response: &agent_client_protocol::v1::schema::InitializeResponse) -> Self {
        Self {
            label: response
                .agent_info
                .as_ref()
                .map(|info| info.name.clone())
                .unwrap_or_else(|| "The ACP agent".to_owned()),
            auth_methods: response
                .auth_methods
                .iter()
                .map(|method| method.name().to_owned())
                .collect(),
        }
    }
}

fn map_error(error: agent_client_protocol::Error, facts: &AgentFacts) -> AcpError {
    let message = error.to_string();
    // The protocol has a dedicated code for this, which is far more reliable
    // than matching wording. The text check stays as a fallback for agents that
    // report a login problem with a generic code.
    let lower = message.to_lowercase();
    if error.code == agent_client_protocol::ErrorCode::AUTH_REQUIRED
        || lower.contains("authentication required")
        || lower.contains("login")
    {
        return AcpError::AuthRequired {
            agent: facts.label.clone(),
            methods: facts.auth_methods.clone(),
        };
    }
    if connection_is_gone(&error) {
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

    /// Build the information the client keeps from a permission request.
    fn permission_request(kinds: &[PermissionOptionKind]) -> PermissionRequestInfo {
        PermissionRequestInfo {
            title: "Run tests".into(),
            kind: Some(wire_name(&ToolKind::Execute)),
            options: kinds
                .iter()
                .map(|kind| (wire_name(kind), Some(wire_name(kind))))
                .collect(),
            diffs: Vec::new(),
            locations: Vec::new(),
        }
    }

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
        let info = permission_request(&[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::RejectOnce,
        ]);
        let response = permission_response(&info, PermissionDecision::Approve);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["outcome"]["outcome"], "selected");
        assert_eq!(json["outcome"]["optionId"], "allow_once");
    }

    #[test]
    fn an_allow_always_only_request_is_cancelled() {
        let info = permission_request(&[PermissionOptionKind::AllowAlways]);
        let response = permission_response(&info, PermissionDecision::Approve);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn cancelling_answers_cancelled_not_a_rejection() {
        let info = permission_request(&[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::RejectOnce,
        ]);
        let response = permission_response(&info, PermissionDecision::Cancel);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(
            json["outcome"]["outcome"], "cancelled",
            "cancel must not look like a rejection: {json}"
        );
        assert!(json["outcome"].get("optionId").is_none());
    }

    #[test]
    fn rejection_prefers_reject_once() {
        let info = permission_request(&[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ]);
        let response = permission_response(&info, PermissionDecision::Reject);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["outcome"]["optionId"], "reject_once");
    }
}
