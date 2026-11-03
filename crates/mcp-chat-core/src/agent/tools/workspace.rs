//! Workspace path resolution and the text-file helpers shared by file tools.

use crate::agent::capabilities::{AccessFormat, AccessKind, table};
use crate::db::repo::{conversations, settings};
use crate::{CoreError, Result};
use serde_json::Value;
use sqlx::SqlitePool;
use std::path::{Component, Path, PathBuf};

/// Resolve the configured workspace, falling back to the process working
/// directory. Built-in file tools are confined to this root.
pub fn resolve_workspace_path(raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    let root = path.canonicalize().map_err(|_| {
        CoreError::invalid(format!(
            "Workspace folder is not accessible: {}",
            path.display()
        ))
    })?;
    if !root.is_dir() {
        return Err(CoreError::invalid(format!(
            "Workspace path is not a folder: {}",
            root.display()
        )));
    }
    Ok(simplify_path(&root))
}

pub async fn workspace_root(pool: &SqlitePool) -> Result<PathBuf> {
    let configured = settings::get(pool, "workspace.root")
        .await?
        .filter(|value| !value.trim().is_empty());
    match configured {
        Some(value) => resolve_workspace_path(&value),
        None => Ok(simplify_path(&std::env::current_dir()?.canonicalize()?)),
    }
}

pub async fn workspace_root_for_conversation(
    pool: &SqlitePool,
    conversation_id: &str,
) -> Result<PathBuf> {
    let conversation = conversations::get(pool, conversation_id).await?;
    match conversation
        .workspace_root
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(path) => resolve_workspace_path(path),
        None => workspace_root(pool).await,
    }
}

pub(super) fn normalize_lexically(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

#[cfg(windows)]
pub(super) fn simplify_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

#[cfg(not(windows))]
pub(super) fn simplify_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(windows)]
pub(super) fn path_is_within(root: &Path, path: &Path) -> bool {
    let root = simplify_path(root);
    let path = simplify_path(path);
    let mut root_parts = root.components();
    let mut path_parts = path.components();
    loop {
        match (root_parts.next(), path_parts.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(left), Some(right)) => {
                if !left
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
                {
                    return false;
                }
            }
        }
    }
}

#[cfg(not(windows))]
pub(super) fn path_is_within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

pub(crate) fn resolve_in_workspace(workspace: &Path, raw: &str) -> Result<PathBuf> {
    if raw.trim().is_empty() {
        return Err(CoreError::invalid("filePath is required"));
    }
    let input = PathBuf::from(raw);
    let joined = if input.is_absolute() {
        input
    } else {
        workspace.join(input)
    };
    let normalized = simplify_path(&normalize_lexically(&joined));
    if !path_is_within(workspace, &normalized) {
        return Err(CoreError::invalid(
            "Path is outside the selected workspace folder.",
        ));
    }
    if normalized.exists() {
        let canonical = simplify_path(&normalized.canonicalize()?);
        if !path_is_within(workspace, &canonical) {
            return Err(CoreError::invalid(
                "Path resolves outside the selected workspace folder.",
            ));
        }
        return Ok(canonical);
    }
    let mut ancestor = normalized.parent();
    while let Some(path) = ancestor {
        if path.exists() {
            let canonical = simplify_path(&path.canonicalize()?);
            if !path_is_within(workspace, &canonical) {
                return Err(CoreError::invalid(
                    "Path resolves outside the selected workspace folder.",
                ));
            }
            break;
        }
        ancestor = path.parent();
    }
    Ok(normalized)
}

/// Read-only tools may leave the workspace in auto mode or after an explicit
/// per-call approval. Writes and shell commands still use the workspace-confined
/// resolver.
pub(crate) fn resolve_read_path(
    workspace: &Path,
    raw: &str,
    allow_outside: bool,
) -> Result<PathBuf> {
    if !allow_outside {
        return resolve_in_workspace(workspace, raw);
    }
    if raw.trim().is_empty() {
        return Err(CoreError::invalid("path is required"));
    }
    let input = PathBuf::from(raw);
    let joined = if input.is_absolute() {
        input
    } else {
        workspace.join(input)
    };
    normalize_lexically(&joined)
        .canonicalize()
        .map(|path| simplify_path(&path))
        .map_err(|_| CoreError::invalid(format!("Path not found: {raw}")))
}

/// True when the target is outside the workspace, including symlink escapes.
pub(crate) fn path_is_outside(workspace: &Path, raw: &str) -> bool {
    if raw.trim().is_empty() {
        return false;
    }
    let input = PathBuf::from(raw);
    let joined = if input.is_absolute() {
        input
    } else {
        workspace.join(input)
    };
    let normalized = simplify_path(&normalize_lexically(&joined));
    if !path_is_within(workspace, &normalized) {
        return true;
    }
    if normalized.exists() {
        return normalized
            .canonicalize()
            .map(|path| !path_is_within(workspace, &path))
            .unwrap_or(true);
    }
    let mut ancestor = normalized.parent();
    while let Some(path) = ancestor {
        if path.exists() {
            return path
                .canonicalize()
                .map(|path| !path_is_within(workspace, &path))
                .unwrap_or(true);
        }
        ancestor = path.parent();
    }
    false
}

/// The path argument of a read-only tool, per the capability table.
pub(super) fn outside_read_target<'a>(name: &str, args: &'a Value) -> Option<&'a str> {
    let access = table().get(name)?.access.as_ref()?;
    if access.kind != AccessKind::Read || access.format != AccessFormat::Path {
        return None;
    }
    args.get(&access.arg).and_then(Value::as_str)
}

/// Writes stay confined unless the user approved a specific outside target.
pub(crate) fn resolve_write_path(
    workspace: &Path,
    raw: &str,
    allow_outside: bool,
) -> Result<PathBuf> {
    if !allow_outside {
        return resolve_in_workspace(workspace, raw);
    }
    if raw.trim().is_empty() {
        return Err(CoreError::invalid("filePath is required"));
    }
    let input = PathBuf::from(raw);
    let joined = if input.is_absolute() {
        input
    } else {
        workspace.join(input)
    };
    let normalized = simplify_path(&normalize_lexically(&joined));
    if normalized.exists() {
        Ok(simplify_path(&normalized.canonicalize()?))
    } else {
        Ok(normalized)
    }
}

pub(super) const MAX_TEXT_BYTES: u64 = 2_000_000;

pub(super) async fn read_text_file(path: &Path) -> Result<String> {
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(CoreError::invalid("Path is not a file."));
    }
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(CoreError::invalid(format!(
            "File is too large ({} bytes); limit is {} bytes.",
            metadata.len(),
            MAX_TEXT_BYTES
        )));
    }
    let bytes = tokio::fs::read(path).await?;
    if bytes.contains(&0) {
        return Err(CoreError::invalid(
            "File looks binary (contains NUL bytes).",
        ));
    }
    String::from_utf8(bytes).map_err(|_| CoreError::invalid("File is not valid UTF-8."))
}

pub(crate) fn display_relative(root: &Path, path: &Path) -> String {
    let root = simplify_path(root);
    let path = simplify_path(path);
    if path_is_within(&root, &path) {
        let root_len = root.components().count();
        let relative: PathBuf = path.components().skip(root_len).collect();
        relative.to_string_lossy().replace('\\', "/")
    } else {
        path.to_string_lossy().replace('\\', "/")
    }
}
