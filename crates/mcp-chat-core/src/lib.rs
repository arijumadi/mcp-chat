pub mod agent;
pub mod attachments;
pub mod builtin_skills;
pub mod db;
pub mod error;
pub mod events;
pub mod llm;
pub mod mcp;
pub mod models;
pub mod secrets;
pub mod updates;
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

pub const DEFAULT_SYSTEM_PROMPT: &str = "You are Jarvis (Jumari Advanced Virtual Intelligence System). You are designed to assist users with a wide range of questions and tasks. You must provide answers that are easy to understand, simple, on point and accurate. Ensure that your answers are relevant to the context and do not contain incorrect information. You will interact with users in a polite and friendly manner. You should analyze the question first and then fulfill users request to the best of your ability.";
