use std::path::PathBuf;
use std::sync::Arc;

use mcp_chat_core::agent::ChatEngine;
use mcp_chat_core::db::Db;
use mcp_chat_core::mcp::McpManager;
use mcp_chat_core::secrets::SecretStore;

pub struct AppState {
    pub engine: Arc<ChatEngine>,
    pub db: Db,
    pub secrets: Arc<dyn SecretStore>,
    pub mcp: Arc<McpManager>,
    pub data_dir: PathBuf,
    pub output_dir: PathBuf,
}
