//! Agent Client Protocol (ACP) support.
//!
//! Phase 1 only holds configuration types: the agent registry row shape lives
//! in `db::repo::acp_agents`, while policy and preset data that does not touch
//! SQLite lives here.

pub mod config;

pub use config::{
    PermissionKind, PermissionOption, PermissionOutcome, PermissionPolicy, auto_decide,
    select_option,
};
