use tauri::State;

use mcp_chat_core::attachments::{self as attachment_files, Attachment};

use crate::error::{CmdResult, cmd_err};
use crate::state::AppState;

#[tauri::command]
pub async fn stage_attachments(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> CmdResult<Vec<Attachment>> {
    attachment_files::stage(&state.engine.attachments_dir, &paths)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn stage_attachment_bytes(
    state: State<'_, AppState>,
    data: String,
    name: Option<String>,
) -> CmdResult<Attachment> {
    attachment_files::stage_image(&state.engine.attachments_dir, &data, name.as_deref())
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn discard_staged_attachments(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> CmdResult<()> {
    attachment_files::discard_pending(&state.engine.attachments_dir, &ids).await;
    Ok(())
}
