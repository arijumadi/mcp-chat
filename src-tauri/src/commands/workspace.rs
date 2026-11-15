use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;
use tokio::fs;
use tokio::io::AsyncReadExt;

use mcp_chat_core::agent::tools::workspace_root_for_conversation;

use crate::error::{CmdResult, cmd_err};
use crate::state::AppState;

const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_ENTRIES: usize = 2_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    pub name: String,
    pub path: String,
    pub kind: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFile {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub truncated: bool,
    pub binary: bool,
}

fn skipped_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".venv"
            | "venv"
            | "__pycache__"
    )
}

async fn workspace_root(state: &AppState, conversation_id: &str) -> CmdResult<PathBuf> {
    workspace_root_for_conversation(&state.db.pool, conversation_id)
        .await
        .map_err(cmd_err)
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn safe_path(root: &Path, raw: &str) -> CmdResult<PathBuf> {
    if raw.contains('\0') {
        return Err("Invalid file path.".into());
    }
    let input = Path::new(raw);
    if input.is_absolute() {
        return Err("File path must be relative to the workspace.".into());
    }
    let joined = root.join(input);
    let canonical = joined
        .canonicalize()
        .map_err(|_| "File or folder was not found.".to_string())?;
    if !canonical.starts_with(root) {
        return Err("File path is outside the conversation workspace.".into());
    }
    Ok(canonical)
}

#[tauri::command]
pub async fn list_workspace_dir(
    state: State<'_, AppState>,
    conversation_id: String,
    path: Option<String>,
) -> CmdResult<Vec<WorkspaceEntry>> {
    let root = workspace_root(&state, &conversation_id).await?;
    let dir = match path.as_deref().filter(|value| !value.trim().is_empty()) {
        Some(value) => safe_path(&root, value)?,
        None => root.clone(),
    };
    if !dir.is_dir() {
        return Err("Path is not a folder.".into());
    }

    let mut entries = Vec::new();
    let mut reader = fs::read_dir(&dir)
        .await
        .map_err(|error| error.to_string())?;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && name != ".mcp-chat" {
            continue;
        }
        let relative = relative_path(&root, &entry.path());
        let is_runtime_dir = relative == ".mcp-chat/artifacts" || relative == ".mcp-chat/tmp";
        if is_runtime_dir {
            continue;
        }
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        let kind = if metadata.is_dir() {
            if skipped_dir(&name) {
                continue;
            }
            "dir"
        } else if metadata.is_file() {
            "file"
        } else {
            continue;
        };
        entries.push(WorkspaceEntry {
            name,
            path: relative,
            kind,
        });
    }
    entries.sort_by(|left, right| {
        (left.kind != "dir", left.name.to_lowercase())
            .cmp(&(right.kind != "dir", right.name.to_lowercase()))
    });
    Ok(entries)
}

#[tauri::command]
pub async fn read_workspace_file(
    state: State<'_, AppState>,
    conversation_id: String,
    path: String,
) -> CmdResult<WorkspaceFile> {
    let root = workspace_root(&state, &conversation_id).await?;
    let file = safe_path(&root, &path)?;
    let metadata = fs::metadata(&file)
        .await
        .map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Path is not a file.".into());
    }
    let size = metadata.len();
    let mut bytes = Vec::with_capacity(MAX_FILE_BYTES as usize + 1);
    fs::File::open(&file)
        .await
        .map_err(|error| error.to_string())?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| error.to_string())?;
    let truncated = bytes.len() as u64 > MAX_FILE_BYTES;
    if truncated {
        bytes.truncate(MAX_FILE_BYTES as usize);
    }
    let binary = bytes.contains(&0);
    let content = if binary {
        String::new()
    } else {
        String::from_utf8_lossy(&bytes).to_string()
    };
    Ok(WorkspaceFile {
        path: relative_path(&root, &file),
        content,
        size,
        truncated,
        binary,
    })
}
