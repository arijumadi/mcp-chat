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
pub mod session;

pub use config::AgentPreset;
pub use error::AcpError;
pub use events::{AcpEvent, TurnOutcome, TurnTranscript};
pub use manager::{AcpManager, MAX_ACTIVE_SESSIONS};
pub use session::{AcpSession, PermissionDecision, PermissionHandler, PermissionRequestInfo};

pub use crate::db::repo::acp_agents::{
    PermissionKind, PermissionOption, PermissionOutcome, PermissionPolicy, auto_decide,
    select_option,
};
