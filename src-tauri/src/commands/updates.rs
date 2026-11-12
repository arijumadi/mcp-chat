use mcp_chat_core::updates::UpdateInfo;

use crate::error::{CmdResult, cmd_err};

#[tauri::command]
pub async fn check_for_update() -> CmdResult<UpdateInfo> {
    mcp_chat_core::updates::check().await.map_err(cmd_err)
}
