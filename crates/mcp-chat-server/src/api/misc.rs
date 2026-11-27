//! Workspace browsing, attachments, settings, skills, and update checks.
//!
//! Workspace path handling is security-relevant: every path is canonicalized
//! and checked to stay inside the conversation root. That logic is copied from
//! `src-tauri/src/commands/workspace.rs` rather than re-derived.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;
use tokio::io::AsyncReadExt;

use mcp_chat_core::agent::tools;
use mcp_chat_core::attachments::{self, image_mime};
use mcp_chat_core::db::repo::{conversations, permissions, providers, servers, settings};
use mcp_chat_core::secrets::{
    oauth_account, oauth_token_account, provider_account, server_api_key_account,
    server_bearer_account,
};

use super::{err, parse, to_value};
use crate::state::AppState;

const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ENTRIES: usize = 2_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceEntry {
    name: String,
    path: String,
    kind: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceFile {
    path: String,
    content: String,
    size: u64,
    truncated: bool,
    binary: bool,
    mime: Option<String>,
    data_url: Option<String>,
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

async fn conversation_root(state: &AppState, conversation_id: &str) -> Result<PathBuf, String> {
    tools::workspace_root_for_conversation(&state.db.pool, conversation_id)
        .await
        .map_err(err)
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn safe_path(root: &Path, raw: &str) -> Result<PathBuf, String> {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListWorkspaceDir {
    conversation_id: String,
    #[serde(default)]
    path: Option<String>,
}

pub async fn list_workspace_dir(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ListWorkspaceDir = parse(args)?;
    let root = conversation_root(state, &params.conversation_id).await?;
    let dir = match params
        .path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
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
        if relative == ".mcp-chat/artifacts" || relative == ".mcp-chat/tmp" {
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
    to_value(entries)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadWorkspaceFile {
    conversation_id: String,
    path: String,
}

pub async fn read_workspace_file(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ReadWorkspaceFile = parse(args)?;
    let root = conversation_root(state, &params.conversation_id).await?;
    let file = safe_path(&root, &params.path)?;
    let metadata = fs::metadata(&file)
        .await
        .map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Path is not a file.".into());
    }
    let size = metadata.len();
    let mut head = [0u8; 16];
    let head_len = fs::File::open(&file)
        .await
        .map_err(|error| error.to_string())?
        .read(&mut head)
        .await
        .map_err(|error| error.to_string())?;
    if let Some(mime) = image_mime(&head[..head_len]) {
        let data_url = if size > MAX_IMAGE_BYTES {
            None
        } else {
            let mut bytes = Vec::with_capacity(size as usize);
            fs::File::open(&file)
                .await
                .map_err(|error| error.to_string())?
                .take(MAX_IMAGE_BYTES + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| error.to_string())?;
            if bytes.len() as u64 > MAX_IMAGE_BYTES {
                None
            } else {
                Some(format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                ))
            }
        };
        return to_value(WorkspaceFile {
            path: relative_path(&root, &file),
            content: String::new(),
            size,
            truncated: false,
            binary: true,
            mime: Some(mime.to_owned()),
            data_url,
        });
    }

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
    to_value(WorkspaceFile {
        path: relative_path(&root, &file),
        content,
        size,
        truncated,
        binary,
        mime: None,
        data_url: None,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StageAttachments {
    paths: Vec<String>,
}

pub async fn stage_attachments(state: &AppState, args: Value) -> Result<Value, String> {
    let params: StageAttachments = parse(args)?;
    let staged = attachments::stage(&state.engine.attachments_dir, &params.paths)
        .await
        .map_err(err)?;
    to_value(staged)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StageAttachmentBytes {
    data: String,
    #[serde(default)]
    name: Option<String>,
}

pub async fn stage_attachment_bytes(state: &AppState, args: Value) -> Result<Value, String> {
    let params: StageAttachmentBytes = parse(args)?;
    let staged = attachments::stage_image(
        &state.engine.attachments_dir,
        &params.data,
        params.name.as_deref(),
    )
    .await
    .map_err(err)?;
    to_value(staged)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StageUpload {
    data: String,
    #[serde(default)]
    name: Option<String>,
}

/// Accept bytes from a browser, which has no server-side path to hand over.
///
/// The bytes are written to a per-upload directory and then handed to the same
/// `attachments::stage` the desktop uses, so text extraction, PDF parsing, size
/// limits and naming all stay in one place.
pub async fn stage_attachment_upload(state: &AppState, args: Value) -> Result<Value, String> {
    let params: StageUpload = parse(args)?;
    let payload = params
        .data
        .split_once(',')
        .filter(|(prefix, _)| prefix.starts_with("data:"))
        .map(|(_, rest)| rest)
        .unwrap_or(&params.data);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|_| "The upload is not valid base64.".to_string())?;
    if bytes.is_empty() {
        return Err("The file is empty.".into());
    }
    if bytes.len() as u64 > attachments::MAX_TOTAL_BYTES {
        return Err("The file is larger than 25 MB.".into());
    }
    let name = safe_upload_name(params.name.as_deref().unwrap_or("file"));
    let slot = state.data_dir.join("uploads").join(mcp_chat_core::new_id());
    tokio::fs::create_dir_all(&slot)
        .await
        .map_err(|error| error.to_string())?;
    let temp = slot.join(&name);
    tokio::fs::write(&temp, &bytes)
        .await
        .map_err(|error| error.to_string())?;
    let result =
        attachments::stage(&state.engine.attachments_dir, &[temp.display().to_string()]).await;
    let _ = tokio::fs::remove_dir_all(&slot).await;
    let staged = result.map_err(err)?;
    to_value(staged)
}

fn safe_upload_name(raw: &str) -> String {
    let base = Path::new(raw)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "file".into()
    } else {
        trimmed.chars().take(100).collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscardStaged {
    ids: Vec<String>,
}

pub async fn discard_staged_attachments(state: &AppState, args: Value) -> Result<Value, String> {
    let params: DiscardStaged = parse(args)?;
    attachments::discard_pending(&state.engine.attachments_dir, &params.ids).await;
    to_value(())
}

pub async fn list_settings(state: &AppState, _args: Value) -> Result<Value, String> {
    let values = settings::all(&state.db.pool).await.map_err(err)?;
    let map: HashMap<String, String> = values.into_iter().collect();
    to_value(map)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetSetting {
    key: String,
    value: String,
}

pub async fn set_setting(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetSetting = parse(args)?;
    if params.key.trim().is_empty() {
        return Err("Setting key is empty".into());
    }
    settings::set(&state.db.pool, params.key.trim(), &params.value)
        .await
        .map_err(err)?;
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteSetting {
    key: String,
}

pub async fn delete_setting(state: &AppState, args: Value) -> Result<Value, String> {
    let params: DeleteSetting = parse(args)?;
    settings::delete(&state.db.pool, &params.key)
        .await
        .map_err(err)?;
    to_value(())
}

pub async fn delete_all_data(state: &AppState, _args: Value) -> Result<Value, String> {
    let provider_list = providers::list(&state.db.pool).await.map_err(err)?;
    for provider in &provider_list {
        let account = provider
            .api_key_ref
            .clone()
            .unwrap_or_else(|| provider.id.clone());
        let _ = state.secrets.delete(&provider_account(&account)).await;
        let _ = state.secrets.delete(&provider.id).await;
    }

    let server_list = servers::list(&state.db.pool).await.map_err(err)?;
    for server in &server_list {
        let _ = state
            .secrets
            .delete(&server_bearer_account(&server.id))
            .await;
        let _ = state
            .secrets
            .delete(&server_api_key_account(&server.id))
            .await;
        let _ = state.secrets.delete(&oauth_token_account(&server.id)).await;
        if let Some(issuer) = &server.oauth_issuer {
            for suffix in ["access_token", "refresh_token", "client_secret"] {
                let _ = state
                    .secrets
                    .delete(&oauth_account(&server.id, issuer, suffix))
                    .await;
            }
        }
        state.mcp.invalidate(&server.id).await;
    }

    let open_conversations = conversations::list_all(&state.db.pool).await.map_err(err)?;
    for conversation in &open_conversations {
        state.engine.stop(&conversation.id);
    }
    conversations::delete_all(&state.db.pool)
        .await
        .map_err(err)?;
    providers::delete_all(&state.db.pool).await.map_err(err)?;
    servers::delete_all(&state.db.pool).await.map_err(err)?;
    permissions::delete_all(&state.db.pool).await.map_err(err)?;
    settings::delete_all(&state.db.pool).await.map_err(err)?;

    let _ = tokio::fs::remove_dir_all(&state.output_dir).await;
    let _ = tokio::fs::create_dir_all(&state.output_dir).await;
    let _ = tokio::fs::remove_dir_all(&state.engine.attachments_dir).await;
    let _ = tokio::fs::create_dir_all(&state.engine.attachments_dir).await;
    let _ = tokio::fs::remove_dir_all(state.data_dir.join("logs")).await;
    to_value(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    name: String,
    version: String,
    data_dir: String,
    workspace_dir: String,
    platform: String,
    auto_update: bool,
}

pub async fn app_info(state: &AppState, _args: Value) -> Result<Value, String> {
    let workspace_dir = tools::workspace_root(&state.db.pool)
        .await
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    to_value(AppInfo {
        name: "Jarvis".into(),
        version: mcp_chat_core::APP_VERSION.into(),
        data_dir: state.data_dir.display().to_string(),
        workspace_dir,
        platform: std::env::consts::OS.into(),
        // There is no installer to swap out on a server.
        auto_update: false,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillView {
    name: String,
    description: String,
    path: String,
    source: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSkills {
    #[serde(default)]
    workspace_root: Option<String>,
}

pub async fn list_skills(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ListSkills = parse(args)?;
    let workspace = match params
        .workspace_root
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(raw) => tools::resolve_workspace_path(raw).map_err(err)?,
        None => tools::workspace_root(&state.db.pool).await.map_err(err)?,
    };
    let disabled = tools::disabled_skill_names(&state.db.pool)
        .await
        .map_err(err)?;
    let views = tools::skill_catalog_infos(&workspace, &state.output_dir)
        .into_iter()
        .map(|skill| SkillView {
            enabled: !disabled.contains(&skill.name),
            name: skill.name,
            description: skill.description,
            path: skill.path,
            source: skill.source,
        })
        .collect::<Vec<_>>();
    to_value(views)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetSkillEnabled {
    name: String,
    enabled: bool,
}

pub async fn set_skill_enabled(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetSkillEnabled = parse(args)?;
    tools::set_skill_enabled(&state.db.pool, &params.name, params.enabled)
        .await
        .map_err(err)?;
    to_value(())
}

pub async fn check_for_update(_state: &AppState, _args: Value) -> Result<Value, String> {
    let info = mcp_chat_core::updates::check().await.map_err(err)?;
    to_value(info)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenExternal {
    url: String,
}

/// The browser is the UI, so it opens the link itself. This exists so a caller
/// that expects the desktop command fails with a clear message instead of a
/// generic 501.
pub async fn open_external(_state: &AppState, args: Value) -> Result<Value, String> {
    let params: OpenExternal = parse(args)?;
    let parsed = url::Url::parse(params.url.trim()).map_err(|_| "Invalid URL".to_string())?;
    if !matches!(parsed.scheme(), "https" | "http" | "mailto") {
        return Err("Only https, http and mailto links can be opened".into());
    }
    to_value(parsed.as_str())
}
