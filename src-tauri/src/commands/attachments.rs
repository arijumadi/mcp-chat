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
pub async fn read_clipboard_image(state: State<'_, AppState>) -> CmdResult<Attachment> {
    let root = state.engine.attachments_dir.clone();
    let png = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
        let image = clipboard.get_image().map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        {
            let mut encoder =
                png::Encoder::new(&mut bytes, image.width as u32, image.height as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
            writer
                .write_image_data(&image.bytes)
                .map_err(|error| error.to_string())?;
        }
        Ok(bytes)
    })
    .await
    .map_err(|error| format!("Clipboard task failed: {error}"))?
    .map_err(|error| format!("Clipboard image unavailable: {error}"))?;
    attachment_files::stage_image_bytes(&root, &png, Some("clipboard.png"))
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
