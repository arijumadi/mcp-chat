//! Shared state for the HTTP server.
//!
//! This mirrors `src-tauri/src/state.rs` so the command bodies can be ported
//! almost verbatim. Nothing here is Tauri-specific, which is the point: the same
//! engine, the same database, the same MCP manager.

use std::path::PathBuf;
use std::sync::Arc;

use mcp_chat_core::agent::ChatEngine;
use mcp_chat_core::db::Db;
use mcp_chat_core::mcp::McpManager;
use mcp_chat_core::secrets::SecretStore;

use crate::auth::Auth;

pub struct AppState {
    pub engine: Arc<ChatEngine>,
    pub db: Db,
    pub secrets: Arc<dyn SecretStore>,
    pub mcp: Arc<McpManager>,
    pub data_dir: PathBuf,
    pub output_dir: PathBuf,
    pub static_dir: PathBuf,
    pub auth: Auth,
}
