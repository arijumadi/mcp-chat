//! Agent Client Protocol (ACP) support for external subagent backends.
//!
//! Layering:
//!
//! - [`config`] holds permission policy and non-mutating agent presets.
//! - [`error`] holds the structured failures the main agent sees.
//! - [`events`] holds agent-neutral updates, so the SDK's types stay inside
//!   this module.
//! - [`process`] owns process launch and tree termination.
//! - [`session`] owns one persistent session and its turn pump.
//! - [`manager`] owns the live session registry and its limits.

pub mod config;
pub mod error;
pub mod events;
pub mod manager;
pub mod mock_agent;
pub mod process;
pub mod prompt;
pub mod session;

pub use config::AgentPreset;
pub use error::AcpError;
pub use events::{AcpEvent, TurnOutcome, TurnTranscript};
pub use manager::{AcpManager, MAX_ACTIVE_SESSIONS};
pub use session::{AcpSession, PermissionDecision, PermissionHandler, PermissionRequestInfo};

/// Result of probing a configured agent, used by the settings UI.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpProbe {
    pub ok: bool,
    pub message: String,
    pub session_id: Option<String>,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
}

pub use crate::db::repo::acp_agents::{
    PermissionKind, PermissionOption, PermissionOutcome, PermissionPolicy, auto_decide,
    select_option,
};
