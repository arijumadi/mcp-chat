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

/// Upper bound on concurrently running agents.
///
/// This is a process-safety limit, not a scheduling one: each session keeps an
/// OS process and a pump task alive, and a runaway model must not be able to
/// fill the process table by delegating.
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

/// A session plus its event stream.
pub struct ActiveSession {
    pub handle: Arc<AcpSession>,
    pub events: mpsc::UnboundedReceiver<AcpEvent>,
}

/// One registered session. The receiver is parked here between turns: a child
/// conversation may run many turns against a single process, and taking it out
/// for the duration of a turn keeps ownership obvious.
struct Entry {
    handle: Arc<AcpSession>,
    events: Option<mpsc::UnboundedReceiver<AcpEvent>>,
}

#[derive(Default)]
pub struct AcpManager {
    sessions: Mutex<HashMap<String, Entry>>,
}

impl AcpManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of sessions whose pump is still running.
    pub async fn count(&self) -> usize {
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, entry| entry.handle.is_alive());
        sessions.len()
    }

    /// Start a session for `local_id`, which is the child conversation id.
    pub async fn open(&self, spec: SessionSpec<'_>) -> Result<ActiveSession, AcpError> {
        let local_id = spec.local_id.clone();
        {
            let mut sessions = self.sessions.lock().await;
            sessions.retain(|_, entry| entry.handle.is_alive());
            check_capacity(sessions.len(), sessions.contains_key(&local_id))
                .map_err(CapacityError::into_acp_error)?;
        }

        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let handle = Arc::new(open(spec, events_tx).await?);

        self.sessions.lock().await.insert(
            local_id,
            Entry {
                handle: handle.clone(),
                events: None,
            },
        );

        Ok(ActiveSession {
            handle,
            events: events_rx,
        })
    }

    /// Take a live session and its event stream for one turn.
    ///
    /// Returns `None` when no live session exists, so the caller starts a new
    /// one. A session that died is removed on the way.
    pub async fn take(&self, local_id: &str) -> Option<ActiveSession> {
        let mut sessions = self.sessions.lock().await;
        let entry = sessions.get_mut(local_id)?;
        if !entry.handle.is_alive() {
            sessions.remove(local_id);
            return None;
        }
        let events = entry.events.take()?;
        Some(ActiveSession {
            handle: entry.handle.clone(),
            events,
        })
    }

    /// Give the event stream back after a turn.
    pub async fn put_events(&self, local_id: &str, events: mpsc::UnboundedReceiver<AcpEvent>) {
        if let Some(entry) = self.sessions.lock().await.get_mut(local_id) {
            entry.events = Some(events);
        }
    }

    /// The live session for `local_id`, dropping it if its process is gone.
    pub async fn get(&self, local_id: &str) -> Option<Arc<AcpSession>> {
        let mut sessions = self.sessions.lock().await;
        match sessions.get(local_id) {
            Some(entry) if entry.handle.is_alive() => Some(entry.handle.clone()),
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
        let entry = self.sessions.lock().await.remove(local_id);
        match entry {
            Some(entry) => entry.handle.close(),
            None => Ok(()),
        }
    }

    pub async fn close_all(&self) {
        let sessions = std::mem::take(&mut *self.sessions.lock().await);
        for (_, entry) in sessions {
            let _ = entry.handle.close();
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

    #[tokio::test]
    async fn taking_an_unknown_session_returns_nothing() {
        let manager = AcpManager::new();
        assert!(manager.take("missing").await.is_none());
    }
}
