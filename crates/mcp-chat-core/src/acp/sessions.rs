//! Listing the sessions an agent already has.
//!
//! ACP exposes `session/list` behind `sessionCapabilities.list`. It is the only
//! way to see conversations that were created outside Jarvis, so this module is
//! deliberately read-only.
//!
//! It goes through [`AcpManager`] rather than calling `session::open` directly,
//! because that is what enforces the four-session cap and, more importantly,
//! what releases a slot. This function closes the session on every path out,
//! including the error paths: an open listing used to hold a slot, so a few
//! visits to the picker left no room to attach anything.
//!
//! Two things the protocol does not settle, learned by probing the real agents:
//!
//! - `cwd` is a filter, and the meaning differs. OpenCode rejects a `load` whose
//!   `cwd` differs from the session's own; Antigravity accepts it but *moves* the
//!   session. Jarvis never rewrites a session's directory: whatever `cwd` the
//!   agent reports is what a later load uses.
//! - Coverage is the agent's decision. Hermes only lists sessions it created
//!   through ACP, so its list is small; Antigravity and OpenCode include the
//!   ones from their own CLI.

use std::path::Path;

use agent_client_protocol::v1::schema::ListSessionsRequest;

use super::error::AcpError;
use super::manager::AcpManager;
use super::session::{Attach, PermissionDecision, PermissionHandler, SessionSpec};

/// One session as the agent describes it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub session_id: String,
    /// The directory the session belongs to. Load must use exactly this.
    pub cwd: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

/// A page of sessions plus the cursor for the next one.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionPage {
    pub sessions: Vec<AgentSession>,
    pub next_cursor: Option<String>,
    /// False when the agent does not advertise `session/list` at all, which is
    /// different from an empty list.
    pub supported: bool,
}

/// Ask an agent for one page of its sessions.
///
/// `cwd` filters the list; pass `None` for everything the agent will show. The
/// process is always closed before returning, whether the query worked or not.
pub async fn list_sessions(
    manager: &AcpManager,
    command: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: Option<&Path>,
    cursor: Option<String>,
    timeout: std::time::Duration,
) -> Result<AgentSessionPage, AcpError> {
    // The process working directory, not a session filter.
    let process_cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/"));
    let no_permissions: PermissionHandler =
        std::sync::Arc::new(|_| Box::pin(async { PermissionDecision::Cancel }));

    // A unique local id per call: a duplicate would be rejected by the manager
    // and, worse, could collide with a conversation id.
    let local_id = format!("list-sessions-{}", crate::now_ms());
    let active = manager
        .open(SessionSpec {
            local_id: local_id.clone(),
            command,
            args,
            env,
            cwd: &process_cwd,
            attach: Attach::New,
            permissions: no_permissions,
        })
        .await?;

    let outcome = ask(&active.handle, cwd, cursor, timeout, command).await;
    // Closing is what frees the slot; it happens even when the query failed.
    let _ = manager.close(&local_id).await;
    outcome
}

async fn ask(
    session: &super::session::AcpSession,
    cwd: Option<&Path>,
    cursor: Option<String>,
    timeout: std::time::Duration,
    command: &str,
) -> Result<AgentSessionPage, AcpError> {
    if !session.supports_listing {
        return Ok(AgentSessionPage {
            sessions: Vec::new(),
            next_cursor: None,
            supported: false,
        });
    }

    let mut request = ListSessionsRequest::new();
    request.cwd = cwd.map(Path::to_path_buf);
    request.cursor = cursor;

    match tokio::time::timeout(
        timeout,
        session.connection().send_request(request).block_task(),
    )
    .await
    {
        Ok(Ok(response)) => Ok(AgentSessionPage {
            sessions: response
                .sessions
                .iter()
                .map(|info| AgentSession {
                    session_id: info.session_id.to_string(),
                    cwd: info.cwd.display().to_string(),
                    title: info.title.clone(),
                    updated_at: info.updated_at.clone(),
                })
                .collect(),
            next_cursor: response.next_cursor.clone(),
            supported: true,
        }),
        // Report the reason rather than an empty list that reads as "you have
        // no sessions".
        Ok(Err(error)) => Err(AcpError::SpawnFailed {
            command: command.to_owned(),
            message: error.to_string(),
        }),
        Err(_) => Err(AcpError::SpawnFailed {
            command: command.to_owned(),
            message: "the agent did not answer session/list in time".into(),
        }),
    }
}
