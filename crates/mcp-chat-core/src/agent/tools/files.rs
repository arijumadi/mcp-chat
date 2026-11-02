//! File tools: read, write, edit, grep and glob.

use super::guards::WRITE_EXISTS_MESSAGE;
use super::workspace::{
    MAX_TEXT_BYTES, display_relative, read_text_file, resolve_read_path, resolve_write_path,
    workspace_root_for_conversation,
};
use super::{Rendered, ToolExecRequest, ToolExecutor};
use crate::util::truncate_chars;
use crate::{CoreError, Result};
use globset::GlobBuilder;
use regex::RegexBuilder;
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use walkdir::WalkDir;

pub(super) fn skip_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    !matches!(
        name.as_ref(),
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".venv"
            | "venv"
            | ".next"
            | ".mcp-chat"
            | ".jarvis"
    )
}

#[derive(Clone)]
pub(super) struct IgnoreRule {
    pub(super) negated: bool,
    pub(super) matcher: globset::GlobMatcher,
}

#[derive(Clone, Default)]
pub(super) struct IgnoreRules {
    pub(super) rules: Vec<IgnoreRule>,
    pub(super) has_negation: bool,
}

impl IgnoreRules {
    pub(super) fn load(workspace: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(workspace.join(".jarvisignore")) else {
            return Self::default();
        };
        let mut rules = Vec::new();
        let mut has_negation = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let negated = line.starts_with('!');
            let raw = line.trim_start_matches('!');
            let anchored = raw.starts_with('/');
            let pattern = raw.trim_start_matches('/').trim_end_matches('/');
            if pattern.is_empty() {
                continue;
            }
            has_negation |= negated;
            let mut patterns = vec![pattern.to_owned()];
            patterns.push(format!("{pattern}/**"));
            if !anchored && !pattern.contains('/') {
                patterns.push(format!("**/{pattern}"));
                patterns.push(format!("**/{pattern}/**"));
            }
            for pattern in patterns {
                if let Ok(glob) = GlobBuilder::new(&pattern).literal_separator(true).build() {
                    rules.push(IgnoreRule {
                        negated,
                        matcher: glob.compile_matcher(),
                    });
                }
            }
        }
        Self {
            rules,
            has_negation,
        }
    }

    pub(super) fn matches(&self, relative: &str) -> bool {
        let relative = relative.trim_start_matches('/');
        let mut candidates = vec![relative.to_owned()];
        let mut path = relative;
        while let Some((parent, _)) = path.rsplit_once('/') {
            candidates.push(parent.to_owned());
            path = parent;
        }
        let mut ignored = false;
        for rule in &self.rules {
            if candidates
                .iter()
                .any(|candidate| rule.matcher.is_match(candidate))
            {
                ignored = !rule.negated;
            }
        }
        ignored
    }

    pub(super) fn has_negation(&self) -> bool {
        self.has_negation
    }
}

pub(super) fn is_text_extension(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "markdown"
            | "rst"
            | "py"
            | "rs"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "json"
            | "jsonc"
            | "toml"
            | "yaml"
            | "yml"
            | "csv"
            | "tsv"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sql"
            | "sh"
            | "bash"
            | "ps1"
            | "cmd"
            | "bat"
            | "xml"
            | "log"
    )
}

pub(super) fn glob_matcher(pattern: &str) -> Result<globset::GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| CoreError::invalid(format!("Invalid glob: {error}")))
}

pub(super) fn line_prefix(content: &str, start_line: usize) -> String {
    let mut output = String::new();
    for (index, line) in content.lines().enumerate() {
        let line = truncate_chars(line, 1000).0;
        output.push_str(&format!("{:>5} | {line}\n", start_line + index));
    }
    output
}

pub(super) fn strip_utf8_bom(value: &str) -> &str {
    value.strip_prefix('\u{feff}').unwrap_or(value)
}

impl ToolExecutor {
    pub(super) async fn run_read(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let raw = req
            .args
            .get("filePath")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("filePath is required"))?;
        let offset = req
            .args
            .get("offset")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1) as usize;
        let limit = req
            .args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(2_000)
            .clamp(1, 5_000) as usize;
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let path = resolve_read_path(
            &workspace,
            raw,
            req.auto_approve || req.allow_outside_workspace,
        )?;
        let relative = display_relative(&workspace, &path);
        let mut head = [0_u8; 32];
        let mut file = tokio::fs::File::open(&path).await?;
        let read = file.read(&mut head).await?;
        if let Some(mime) = crate::attachments::image_mime(&head[..read]) {
            let attachment =
                crate::attachments::store_read_image(&self.attachments_dir, &path).await?;
            return Ok(Rendered {
                content: format!(
                    "Image loaded: {} ({}, {} bytes)",
                    relative, mime, attachment.size
                ),
                structured: Some(json!({"image": attachment})),
                ok: true,
            });
        }
        let content = read_text_file(&path).await?;
        let content = strip_utf8_bom(&content);
        let lines = content.lines().collect::<Vec<_>>();
        let total = lines.len();
        if total == 0 {
            return Ok(Rendered {
                content: format!("{relative} is empty."),
                structured: None,
                ok: true,
            });
        }
        if offset > total {
            return Ok(Rendered {
                content: format!(
                    "{relative} has {total} line(s); offset {offset} is past the end."
                ),
                structured: None,
                ok: true,
            });
        }
        let end = (offset - 1 + limit).min(total);
        let selected = lines[offset - 1..end].join("\n");
        let body = line_prefix(&selected, offset);
        Ok(Rendered {
            content: format!("{relative} (lines {offset}-{end} of {total})\n{body}"),
            structured: None,
            ok: true,
        })
    }

    pub(super) async fn run_write(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let raw = req
            .args
            .get("filePath")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("filePath is required"))?;
        let content = req
            .args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("content is required"))?;
        let overwrite = req
            .args
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if content.len() as u64 > MAX_TEXT_BYTES {
            return Err(CoreError::invalid("File content is too large."));
        }
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let path = resolve_write_path(&workspace, raw, req.allow_outside_workspace)?;
        let _guard = self.file_locks.acquire(vec![path.clone()]).await;
        let relative = display_relative(&workspace, &path);
        // Nothing is written, and no folder is created, when the target exists.
        if !overwrite
            && tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.is_file())
        {
            return Err(CoreError::invalid(WRITE_EXISTS_MESSAGE));
        }
        // Capture the previous text before overwriting. A missing file diffs
        // against empty; an unreadable (binary or oversized) one gets no diff
        // and must not fail the write.
        let previous = match tokio::fs::metadata(&path).await {
            Ok(_) => read_text_file(&path).await.ok(),
            Err(_) => Some(String::new()),
        };
        let diff =
            previous.map(|previous| crate::agent::diff::file_diff(&relative, &previous, content));
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if overwrite {
            tokio::fs::write(&path, content).await?;
        } else {
            // create_new keeps this safe against a file appearing after the check.
            let mut file = match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(CoreError::invalid(WRITE_EXISTS_MESSAGE));
                }
                Err(error) => return Err(error.into()),
            };
            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
        }
        Ok(Rendered {
            content: format!("Wrote {relative} ({} bytes).", content.len()),
            structured: Some(json!({"path": relative, "bytes": content.len(), "diff": diff})),
            ok: true,
        })
    }

    pub(super) async fn run_edit(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let raw = req
            .args
            .get("filePath")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("filePath is required"))?;
        let old = req
            .args
            .get("oldString")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("oldString is required"))?;
        let new = req
            .args
            .get("newString")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("newString is required"))?;
        if old.is_empty() {
            return Err(CoreError::invalid("oldString must not be empty."));
        }
        let replace_all = req
            .args
            .get("replaceAll")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let path = resolve_write_path(&workspace, raw, req.allow_outside_workspace)?;
        let _guard = self.file_locks.acquire(vec![path.clone()]).await;
        let content = read_text_file(&path).await?;
        let relative = display_relative(&workspace, &path);
        let outcome = crate::agent::edit::apply(&content, old, new, replace_all)
            .map_err(|error| CoreError::invalid(format!("{relative}: {}", error.user_message())))?;
        if outcome.content.len() as u64 > MAX_TEXT_BYTES {
            return Err(CoreError::invalid("Edited file is too large."));
        }
        // Built from the in-memory before/after, never by re-reading the file.
        let diff = crate::agent::diff::file_diff(&relative, &content, &outcome.content);
        tokio::fs::write(&path, &outcome.content).await?;
        Ok(Rendered {
            content: format!(
                "Edited {relative}: replaced {} occurrence(s).",
                outcome.replacements
            ),
            structured: Some(json!({
                "path": relative,
                "replacements": outcome.replacements,
                "diff": diff
            })),
            ok: true,
        })
    }

    pub(super) async fn run_grep(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let pattern = req
            .args
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("pattern is required"))?
            .to_owned();
        let case_sensitive = req
            .args
            .get("caseSensitive")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let limit = req
            .args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(200)
            .clamp(1, 1_000) as usize;
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let search = resolve_read_path(
            &workspace,
            req.args.get("path").and_then(Value::as_str).unwrap_or("."),
            req.auto_approve || req.allow_outside_workspace,
        )?;
        let include = req
            .args
            .get("include")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(glob_matcher)
            .transpose()?;
        let has_include = include.is_some();
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|error| CoreError::invalid(format!("Invalid regular expression: {error}")))?;
        let root = workspace.clone();
        let found = tokio::task::spawn_blocking(move || -> Result<String> {
            let ignore = IgnoreRules::load(&root);
            let filter_root = root.clone();
            let filter_ignore = ignore.clone();
            let mut hits = Vec::new();
            let mut truncated = false;
            'files: for entry in WalkDir::new(&search)
                .follow_links(false)
                .into_iter()
                .filter_entry(move |entry| {
                    if !skip_dir(entry) {
                        return false;
                    }
                    if entry.file_type().is_dir() {
                        let relative = display_relative(&filter_root, entry.path());
                        return !filter_ignore.matches(&relative) || filter_ignore.has_negation();
                    }
                    true
                })
            {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let relative = display_relative(&root, path);
                if ignore.matches(&relative) {
                    continue;
                }
                if !has_include && !is_text_extension(path) {
                    continue;
                }
                if let Some(include) = &include {
                    if !include.is_match(&relative) {
                        continue;
                    }
                }
                let metadata = match entry.metadata() {
                    Ok(metadata) => metadata,
                    Err(_) => continue,
                };
                if metadata.len() > MAX_TEXT_BYTES {
                    continue;
                }
                let bytes = match std::fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };
                if bytes.contains(&0) {
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes);
                for (index, line) in text.lines().enumerate() {
                    if regex.is_match(line) {
                        hits.push(format!(
                            "{relative}:{}: {}",
                            index + 1,
                            truncate_chars(line, 300).0
                        ));
                        if hits.len() >= limit {
                            truncated = true;
                            break 'files;
                        }
                    }
                }
            }
            if hits.is_empty() {
                Ok(format!("No matches for /{pattern}/."))
            } else {
                Ok(format!(
                    "{} matching line(s){}:\n{}",
                    hits.len(),
                    if truncated { " (limit reached)" } else { "" },
                    hits.join("\n")
                ))
            }
        })
        .await
        .map_err(|error| CoreError::Message(format!("grep failed: {error}")))??;
        Ok(Rendered {
            content: found,
            structured: None,
            ok: true,
        })
    }

    pub(super) async fn run_glob(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let pattern = req
            .args
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("pattern is required"))?
            .to_owned();
        let limit = req
            .args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(500)
            .clamp(1, 2_000) as usize;
        let recursive = req
            .args
            .get("recursive")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let search = resolve_read_path(
            &workspace,
            req.args.get("path").and_then(Value::as_str).unwrap_or("."),
            req.auto_approve || req.allow_outside_workspace,
        )?;
        let matcher = glob_matcher(&pattern)?;
        let root = workspace.clone();
        let found = tokio::task::spawn_blocking(move || -> Result<String> {
            let ignore = IgnoreRules::load(&root);
            let filter_root = root.clone();
            let filter_ignore = ignore.clone();
            let mut walker = WalkDir::new(&search).follow_links(false);
            if !recursive {
                walker = walker.max_depth(1);
            }
            let mut paths = Vec::new();
            'walk: for entry in walker.into_iter().filter_entry(move |entry| {
                if !skip_dir(entry) {
                    return false;
                }
                if entry.file_type().is_dir() {
                    let relative = display_relative(&filter_root, entry.path());
                    return !filter_ignore.matches(&relative);
                }
                true
            }) {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative = display_relative(&root, entry.path());
                if ignore.matches(&relative) {
                    continue;
                }
                if matcher.is_match(&relative) {
                    paths.push(relative);
                    if paths.len() >= limit {
                        break 'walk;
                    }
                }
            }
            paths.sort();
            if paths.is_empty() {
                Ok(format!("No files matched {pattern}."))
            } else {
                Ok(format!("{} file(s):\n{}", paths.len(), paths.join("\n")))
            }
        })
        .await
        .map_err(|error| CoreError::Message(format!("glob failed: {error}")))??;
        Ok(Rendered {
            content: found,
            structured: None,
            ok: true,
        })
    }
}
