//! Live ACP session registry.
//!
//! Sessions are per-process and never survive a restart. The database marker
//! (`conversations.acp_agent`) is what tells the engine that a stale child was
//! an ACP session rather than an internal subagent.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

use super::error::AcpError;
use super::events::AcpEvent;
use super::session::{AcpSession, SessionSpec, open};

/// Upper bound on concurrently running agents. Matches the executor's
/// parallelism so the model cannot exhaust the process table by delegating.
pub const MAX_ACTIVE_SESSIONS: usize = 4;

/// Why a new session cannot start, before any process is spawned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityError {
    Full,
    Duplicate,
}

/// Pure capacity check, separated so it can be tested without real processes.
pub fn check_capacity(active: usize, already_open: bool) -> Result<(), CapacityError> {
    if already_open {
        return Err(CapacityError::Duplicate);
    }
    if active >= MAX_ACTIVE_SESSIONS {
        return Err(CapacityError::Full);
    }
    Ok(())
}

impl CapacityError {
    pub fn into_acp_error(self) -> AcpError {
        match self {
            Self::Full => AcpError::Protocol {
                message: format!(
                    "At most {MAX_ACTIVE_SESSIONS} ACP sessions can run at once. Close one first."
                ),
            },
            Self::Duplicate => AcpError::Protocol {
                message: "That conversation already has an ACP session.".into(),
            },
        }
    }
}

/// What the engine keeps after a session starts.
pub struct ActiveSession {
    pub handle: Arc<AcpSession>,
    pub events: mpsc::UnboundedReceiver<AcpEvent>,
}

#[derive(Default)]
pub struct AcpManager {
    sessions: Mutex<HashMap<String, Arc<AcpSession>>>,
}

impl AcpManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of sessions whose pump is still running.
    pub async fn count(&self) -> usize {
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, session| session.is_alive());
        sessions.len()
    }

    /// Start a session for `local_id`, which is the child conversation id.
    pub async fn open(&self, spec: SessionSpec<'_>) -> Result<ActiveSession, AcpError> {
        let local_id = spec.local_id.clone();
        {
            let mut sessions = self.sessions.lock().await;
            sessions.retain(|_, session| session.is_alive());
            check_capacity(sessions.len(), sessions.contains_key(&local_id))
                .map_err(CapacityError::into_acp_error)?;
        }

        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let handle = Arc::new(open(spec, events_tx).await?);

        self.sessions.lock().await.insert(local_id, handle.clone());

        Ok(ActiveSession {
            handle,
            events: events_rx,
        })
    }

    /// The live session for `local_id`, dropping it if its process is gone.
    pub async fn get(&self, local_id: &str) -> Option<Arc<AcpSession>> {
        let mut sessions = self.sessions.lock().await;
        match sessions.get(local_id) {
            Some(session) if session.is_alive() => Some(session.clone()),
            Some(_) => {
                sessions.remove(local_id);
                None
            }
            None => None,
        }
    }

    pub async fn cancel(&self, local_id: &str) -> Result<(), AcpError> {
        let session = self.get(local_id).await.ok_or(AcpError::Inactive)?;
        session.cancel()
    }

    /// Close one session, whether or not it is still registered.
    pub async fn close(&self, local_id: &str) -> Result<(), AcpError> {
        let session = self.sessions.lock().await.remove(local_id);
        match session {
            Some(session) => session.close(),
            None => Ok(()),
        }
    }

    pub async fn close_all(&self) {
        let sessions = std::mem::take(&mut *self.sessions.lock().await);
        for (_, session) in sessions {
            let _ = session.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_allows_up_to_the_limit() {
        for active in 0..MAX_ACTIVE_SESSIONS {
            assert_eq!(check_capacity(active, false), Ok(()));
        }
        assert_eq!(
            check_capacity(MAX_ACTIVE_SESSIONS, false),
            Err(CapacityError::Full)
        );
        assert_eq!(
            check_capacity(MAX_ACTIVE_SESSIONS + 5, false),
            Err(CapacityError::Full)
        );
    }

    #[test]
    fn duplicates_win_over_capacity() {
        assert_eq!(
            check_capacity(MAX_ACTIVE_SESSIONS, true),
            Err(CapacityError::Duplicate)
        );
    }

    #[test]
    fn capacity_errors_explain_themselves() {
        let full = CapacityError::Full.into_acp_error().user_message();
        assert!(full.contains("At most"));
        assert!(full.contains("Close one first"));
        let duplicate = CapacityError::Duplicate.into_acp_error().user_message();
        assert!(duplicate.contains("already has an ACP session"));
    }

    #[tokio::test]
    async fn closing_an_unknown_session_is_a_no_op() {
        let manager = AcpManager::new();
        assert!(manager.close("missing").await.is_ok());
        assert_eq!(manager.count().await, 0);
    }

    #[tokio::test]
    async fn cancelling_an_unknown_session_reports_inactive() {
        let manager = AcpManager::new();
        let error = manager.cancel("missing").await.expect_err("inactive");
        assert!(error.user_message().contains("ACP session has ended"));
    }
}
