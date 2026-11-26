//! Structured errors for ACP subagents.
//!
//! These are what the main agent sees when an external session fails, so each
//! variant carries actionable text instead of a raw protocol dump.

use crate::CoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpError {
    /// The configured binary or package runner is not on PATH.
    NotFound { command: String },
    /// The process exists but could not be started.
    SpawnFailed { command: String, message: String },
    /// `initialize` failed or negotiated an unusable version.
    InitFailed { message: String },
    /// The agent needs an interactive login before it will create sessions.
    AuthRequired { agent: String },
    /// The process exited mid-turn. `stderr_tail` is bounded.
    Crashed { stderr_tail: String },
    /// The session exists in the database but its process is gone.
    Inactive,
    /// A protocol-level failure.
    Protocol { message: String },
    /// `session/new` did not return an id.
    NoSession,
}

impl AcpError {
    pub fn user_message(&self) -> String {
        match self {
            Self::NotFound { command } => format!(
                "ACP agent command {command:?} was not found. Install it, or check the command in Settings > ACP agents."
            ),
            Self::SpawnFailed { command, message } => {
                format!("Could not start ACP agent {command:?}: {message}")
            }
            Self::InitFailed { message } => {
                format!("The ACP agent did not initialize: {message}")
            }
            Self::AuthRequired { agent } => format!(
                "ACP agent {agent:?} needs a login first. Sign in with that agent's own CLI, then retry."
            ),
            Self::Crashed { stderr_tail } if stderr_tail.trim().is_empty() => {
                "The ACP agent exited unexpectedly.".to_owned()
            }
            Self::Crashed { stderr_tail } => {
                format!("The ACP agent exited unexpectedly. Last output:\n{stderr_tail}")
            }
            Self::Inactive => {
                "That ACP session has ended (the app was restarted or the process closed). Start a new session."
                    .to_owned()
            }
            Self::Protocol { message } => format!("ACP protocol error: {message}"),
            Self::NoSession => "The ACP agent did not return a session id.".to_owned(),
        }
    }
}

impl From<AcpError> for CoreError {
    fn from(error: AcpError) -> Self {
        CoreError::Message(error.user_message())
    }
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.user_message())
    }
}

impl std::error::Error for AcpError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_actionable_text() {
        let cases = [
            AcpError::NotFound {
                command: "opencode".into(),
            },
            AcpError::SpawnFailed {
                command: "npx".into(),
                message: "permission denied".into(),
            },
            AcpError::InitFailed {
                message: "unknown method".into(),
            },
            AcpError::AuthRequired {
                agent: "codex".into(),
            },
            AcpError::Crashed {
                stderr_tail: "panic: no token".into(),
            },
            AcpError::Crashed {
                stderr_tail: String::new(),
            },
            AcpError::Inactive,
            AcpError::Protocol {
                message: "bad json".into(),
            },
            AcpError::NoSession,
        ];
        for error in cases {
            let text = error.user_message();
            assert!(!text.is_empty(), "empty message for {error:?}");
            assert!(
                text.ends_with('.') || text.contains('\n') || text.contains(' '),
                "message looks terse: {text}"
            );
        }
    }

    #[test]
    fn missing_binary_names_the_command() {
        let error = AcpError::NotFound {
            command: "gemini".into(),
        };
        assert!(error.user_message().contains("gemini"));
    }

    #[test]
    fn crash_includes_the_captured_output() {
        let error = AcpError::Crashed {
            stderr_tail: "session expired".into(),
        };
        assert!(error.user_message().contains("session expired"));
    }

    #[test]
    fn converts_to_a_core_error() {
        let core: CoreError = AcpError::Inactive.into();
        assert!(core.user_message().contains("ACP session has ended"));
    }
}
