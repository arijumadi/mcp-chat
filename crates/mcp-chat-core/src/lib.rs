pub mod agent;
pub mod db;
pub mod error;
pub mod events;
pub mod llm;
pub mod mcp;
pub mod models;
pub mod secrets;
pub mod util;

pub use error::{CoreError, Result};
pub use models::*;

pub fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

pub const APP_NAME: &str = "mcp-chat";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const KEYCHAIN_SERVICE: &str = "com.mcpchat.app";
pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
pub const MCP_LEGACY_VERSION: &str = "2025-11-25";
