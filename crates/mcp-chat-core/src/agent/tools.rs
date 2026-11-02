use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use futures::StreamExt;
use globset::GlobBuilder;
use regex::RegexBuilder;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;

use crate::agent::approvals::{ApprovalDecision, ApprovalRegistry, InputAnswer, InputRegistry};
use crate::agent::file_locks::FileLocks;
use crate::agent::permissions::{self, Effect};
use crate::db::Db;
use crate::db::repo::{conversations, messages, permissions as rules_repo, settings};
use crate::events::CoreEvent;
use crate::llm::ToolDef;
use crate::mcp::manager::McpManager;
use crate::mcp::types::ToolInfo;
use crate::mcp::{MrtrRetry, ProgressEvent};
use crate::models::PartPayload;
use crate::util::truncate_chars;
use crate::{CoreError, Result, now_ms};

const MODEL_OUTPUT_LIMIT: usize = 12_000;
const SHELL_OUTPUT_LIMIT: usize = 16_000;
const READ_OUTPUT_LIMIT: usize = 24_000;
const PS_PROLOG: &str = "$ProgressPreference='SilentlyContinue';$env:PYTHONIOENCODING='utf-8';$env:PYTHONUTF8='1';[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);$OutputEncoding=[Console]::OutputEncoding;";
const READ_PAGE_CHARS: usize = 24_000;
const PREVIEW_LIMIT: usize = 4_000;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone)]
pub struct ToolExecutor {
    pub db: Db,
    pub mcp: Arc<McpManager>,
    pub approvals: Arc<ApprovalRegistry>,
    pub inputs: Arc<InputRegistry>,
    pub events: broadcast::Sender<CoreEvent>,
    pub output_dir: PathBuf,
    pub attachments_dir: PathBuf,
    pub limiter: Arc<Semaphore>,
    pub file_locks: FileLocks,
}

pub struct ToolExecRequest {
    pub conversation_id: String,
    pub message_id: String,
    pub part_id: String,
    pub call_id: String,
    pub index: usize,
    pub exposed_name: String,
    pub remote_name: String,
    pub server_id: String,
    pub server_slug: String,
    pub server_name: String,
    pub args: Value,
    pub tool: Option<ToolInfo>,
    pub default_effect: Effect,
    pub auto_approve: bool,
    /// Set only after the user explicitly approves outside-workspace access.
    pub allow_outside_workspace: bool,
    pub disabled: bool,
    pub cancel: CancellationToken,
    pub active_server_slugs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ToolExecResult {
    pub call_id: String,
    pub index: usize,
    pub exposed_name: String,
    pub remote_name: String,
    pub server_id: String,
    pub server_slug: String,
    pub server_name: String,
    pub ok: bool,
    pub content: String,
    pub structured: Option<Value>,
    pub full_result_ref: Option<String>,
    pub truncated: bool,
    pub state: String,
    pub duration_ms: i64,
    pub executed: bool,
}

pub const MCP_SEARCH_TOOLS: &str = "mcp_search_tools";
pub const MCP_GET_TOOL_SCHEMA: &str = "mcp_get_tool_schema";
pub const MCP_CALL_TOOL: &str = "mcp_call_tool";
pub const MCP_EXECUTE: &str = "mcp_execute";

pub fn is_meta_tool(name: &str) -> bool {
    matches!(
        name,
        MCP_SEARCH_TOOLS | MCP_GET_TOOL_SCHEMA | MCP_CALL_TOOL | MCP_EXECUTE
    )
}

pub fn meta_tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: MCP_SEARCH_TOOLS.into(),
            description: "Look up an MCP tool only when it is missing from the tool catalog in the system prompt. The catalog already lists every active server and tool; do not call this just to browse.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Optional words to match against server, tool or description."},
                    "server": {"type": "string", "description": "Optional server slug to restrict the search."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum results. Default 20."}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: MCP_GET_TOOL_SCHEMA.into(),
            description: "Return the JSON input schema for one MCP tool, identified by its server slug and tool name.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string", "description": "Server slug from mcp_search_tools."},
                    "tool": {"type": "string", "description": "Remote tool name from mcp_search_tools."}
                },
                "required": ["server", "tool"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: MCP_EXECUTE.into(),
            description: "Execute several independent MCP tool calls in one step (max 8, up to 4 in flight). Use this when you need multiple tools for the same answer. Each call still follows its server policy and may ask for approval.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "calls": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 8,
                        "items": {
                            "type": "object",
                            "properties": {
                                "server": {"type": "string"},
                                "tool": {"type": "string"},
                                "arguments": {"type": "object", "additionalProperties": true}
                            },
                            "required": ["server", "tool", "arguments"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["calls"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: MCP_CALL_TOOL.into(),
            description: "Call a remote MCP tool. Use mcp_search_tools to discover it and mcp_get_tool_schema to see its arguments first. The target server policy still applies and may require approval.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string", "description": "Server slug from mcp_search_tools."},
                    "tool": {"type": "string", "description": "Remote tool name from mcp_search_tools."},
                    "arguments": {"type": "object", "description": "Arguments matching the tool's input schema.", "additionalProperties": true}
                },
                "required": ["server", "tool", "arguments"],
                "additionalProperties": false
            }),
        },
    ]
}

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

fn normalize_lexically(path: &Path) -> PathBuf {
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
fn simplify_path(path: &Path) -> PathBuf {
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
fn simplify_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(windows)]
fn path_is_within(root: &Path, path: &Path) -> bool {
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
fn path_is_within(root: &Path, path: &Path) -> bool {
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

fn outside_read_target<'a>(name: &str, args: &'a Value) -> Option<&'a str> {
    match name {
        "read" => args.get("filePath"),
        "grep" | "glob" => args.get("path"),
        _ => None,
    }
    .and_then(Value::as_str)
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

const MAX_TEXT_BYTES: u64 = 2_000_000;

async fn read_text_file(path: &Path) -> Result<String> {
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

async fn skill_listing(dir: &Path, name: &str, relative: &str) -> Result<String> {
    let mut entries = Vec::new();
    let mut reader = tokio::fs::read_dir(dir).await?;
    let mut truncated = false;
    while let Some(entry) = reader.next_entry().await? {
        if entries.len() >= 200 {
            truncated = true;
            break;
        }
        let metadata = entry.metadata().await.ok();
        let file_type = entry.file_type().await.ok();
        let label = entry.file_name().to_string_lossy().replace('\\', "/");
        let kind = if file_type.as_ref().is_some_and(|value| value.is_dir()) {
            "dir"
        } else if file_type.as_ref().is_some_and(|value| value.is_file()) {
            "file"
        } else {
            "link"
        };
        let size = metadata
            .as_ref()
            .map(|value| value.len())
            .unwrap_or_default();
        entries.push((label, kind, size));
    }
    entries.sort_by_key(|entry| entry.0.to_lowercase());
    let label = if relative.is_empty() || relative == "." {
        name.to_owned()
    } else {
        format!("{name}/{relative}")
    };
    let mut out = format!("# Skill: {label}\n");
    for (entry, kind, size) in entries {
        if kind == "dir" {
            out.push_str(&format!("- {entry}/\n"));
        } else if kind == "file" {
            out.push_str(&format!("- {entry} ({size} B)\n"));
        } else {
            out.push_str(&format!("- {entry} (link)\n"));
        }
    }
    if truncated {
        out.push_str("- ... (more entries not shown)\n");
    }
    Ok(out)
}

fn skip_dir(entry: &walkdir::DirEntry) -> bool {
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
struct IgnoreRule {
    negated: bool,
    matcher: globset::GlobMatcher,
}

#[derive(Clone, Default)]
struct IgnoreRules {
    rules: Vec<IgnoreRule>,
    has_negation: bool,
}

impl IgnoreRules {
    fn load(workspace: &Path) -> Self {
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

    fn matches(&self, relative: &str) -> bool {
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

    fn has_negation(&self) -> bool {
        self.has_negation
    }
}

fn is_text_extension(path: &Path) -> bool {
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

fn glob_matcher(pattern: &str) -> Result<globset::GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| CoreError::invalid(format!("Invalid glob: {error}")))
}

fn shell_launch_error(shell: &str, error: std::io::Error) -> CoreError {
    if error.kind() == std::io::ErrorKind::NotFound {
        CoreError::Message(format!(
            "Shell '{shell}' was not found on this system. Install it or choose another shell (cmd, powershell, pwsh, sh or bash)."
        ))
    } else {
        CoreError::Message(format!("Shell '{shell}' could not be started: {error}"))
    }
}

fn decode_clixml_escapes(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'_'
            && index + 7 <= bytes.len()
            && bytes[index + 1] == b'x'
            && bytes[index + 6] == b'_'
        {
            let hex = &value[index + 2..index + 6];
            if hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                if let Ok(code) = u32::from_str_radix(hex, 16) {
                    if let Some(ch) = char::from_u32(code) {
                        output.push(ch);
                        index += 7;
                        continue;
                    }
                }
            }
        }
        let ch = value[index..].chars().next().expect("valid utf-8");
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn extract_clixml_errors(block: &str) -> String {
    let mut output = String::new();
    let mut rest = block;
    while let Some(start) = rest.find("<S S=\"Error\">") {
        let after = &rest[start + "<S S=\"Error\">".len()..];
        let Some(end) = after.find("</S>") else {
            break;
        };
        let text = decode_clixml_escapes(&after[..end]);
        output.push_str(text.trim_end());
        output.push('\n');
        rest = &after[end + "</S>".len()..];
    }
    output
}

/// PowerShell writes progress records to stderr as CLIXML when stderr is
/// redirected. Strip those blocks, but keep serialized error text if present.
fn strip_powershell_prolog(value: &str) -> String {
    let needle = PS_PROLOG.as_bytes();
    let mut output = String::new();
    let mut rest = value;
    loop {
        // Ignore whitespace when locating the prolog so PowerShell's arbitrary
        // line wrapping (and indentation) does not hide the tail.
        let mut compact = Vec::with_capacity(rest.len());
        let mut origins = Vec::with_capacity(rest.len());
        for (index, ch) in rest.char_indices() {
            if ch.is_whitespace() {
                continue;
            }
            let bytes = &rest.as_bytes()[index..index + ch.len_utf8()];
            for (offset, byte) in bytes.iter().enumerate() {
                compact.push(*byte);
                origins.push(index + offset);
            }
        }
        let Some(position) = compact
            .windows(needle.len())
            .position(|window| window == needle)
        else {
            output.push_str(rest);
            break;
        };
        let start = origins[position];
        let end = origins[position + needle.len() - 1] + 1;
        output.push_str(&rest[..start]);
        rest = rest[end..].trim_start_matches(['\r', '\n']);
    }
    output
}

fn clean_powershell_stderr(value: &str) -> String {
    let mut output = String::new();
    let mut rest = value;
    loop {
        let marker = rest.find("#< CLIXML");
        let block = rest.find("<Objs");
        let start = match (marker, block) {
            (Some(marker), Some(block)) => marker.min(block),
            (Some(marker), None) => marker,
            (None, Some(block)) => block,
            (None, None) => {
                output.push_str(rest);
                break;
            }
        };
        output.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(objects) = tail.find("<Objs") else {
            break;
        };
        let Some(end) = tail[objects..].find("</Objs>") else {
            break;
        };
        let block_end = objects + end + "</Objs>".len();
        output.push_str(&extract_clixml_errors(&tail[objects..block_end]));
        rest = &tail[block_end..];
    }
    // The encoding/progress prolog is part of the PowerShell error context on
    // some versions, and PowerShell may wrap it across lines.
    strip_powershell_prolog(&output).trim_end().to_owned()
}

fn normalize_shell_text(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn powershell_script(command: &str) -> String {
    format!(
        "{PS_PROLOG}\n$ErrorActionPreference = 'Stop'\ntry {{\n{command}\nif ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}\nif (-not $?) {{ exit 1 }}\nexit 0\n}} catch {{\n[Console]::Error.WriteLine($_.Exception.Message)\nexit 1\n}}"
    )
}

fn shell_echoed(command: &str, stdout: &str) -> bool {
    !stdout.trim().is_empty() && normalize_shell_text(stdout) == normalize_shell_text(command)
}

/// Only cmd.exe is allowed to trigger the echo heuristic. PowerShell's
/// -EncodedCommand does not echo the script, and a command such as `$ "1+1"`
/// legitimately produces output identical to its command text.
fn echo_suspect(shell: &str, command: &str, stdout: &str) -> bool {
    shell == "cmd" && shell_echoed(command, stdout)
}

fn clip_lines(text: &str, max_lines: usize, tail: bool) -> (String, usize, bool) {
    let total = if text.is_empty() {
        0
    } else {
        text.lines().count()
    };
    if max_lines == 0 || total <= max_lines {
        return (text.to_owned(), total, false);
    }
    let lines: Vec<&str> = text.lines().collect();
    let selected = if tail {
        &lines[total - max_lines..]
    } else {
        &lines[..max_lines]
    };
    (selected.join("\n"), total, true)
}

fn line_prefix(content: &str, start_line: usize) -> String {
    let mut output = String::new();
    for (index, line) in content.lines().enumerate() {
        let line = truncate_chars(line, 1000).0;
        output.push_str(&format!("{:>5} | {line}\n", start_line + index));
    }
    output
}

pub fn meta_tool_info(name: &str) -> Option<ToolInfo> {
    meta_tool_defs()
        .into_iter()
        .find(|definition| definition.name == name)
        .map(|definition| ToolInfo {
            name: definition.name,
            title: None,
            description: definition.description,
            input_schema: definition.input_schema,
            output_schema: None,
            annotations: None,
        })
}

pub fn builtin_tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read_tool_output".into(),
            description: "Read a truncated tool result by call_id. Use offset for character paging, or line + maxLines for line-based paging.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "call_id": {"type": "string", "description": "The tool call id to read."},
                    "offset": {"type": "integer", "minimum": 0, "description": "Character offset. Default 0. Pages end on line boundaries when possible."},
                    "line": {"type": "integer", "minimum": 1, "description": "Optional 1-based line to start from. Takes precedence over offset."},
                    "maxLines": {"type": "integer", "minimum": 1, "maximum": 5000, "description": "Maximum lines when using line-based paging. Default 200."}
                },
                "required": ["call_id"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "bash".into(),
            description: "Run a shell command for build, test, lint, git, package managers, process/system/disk/network inspection, file management (rename/move/delete), or running scripts. Do not use bash to read or modify file contents; use read, write, or apply_patch instead. Before calling bash, ask: does this command change the contents of a file? If yes, use a file tool.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to run. Do not wrap it in another invocation of the same shell."},
                    "shell": {
                        "type": "string",
                        "enum": ["auto", "cmd", "powershell", "pwsh", "sh", "bash"],
                        "description": "Default auto: powershell on Windows, sh elsewhere. For PowerShell send the script body only."
                    },
                    "cwd": {"type": "string", "description": "Optional workspace-relative working directory."},
                    "maxLines": {"type": "integer", "minimum": 1, "maximum": 5000, "description": "Optional maximum lines of stdout/stderr to return."},
                    "tail": {"type": "boolean", "description": "Keep the last maxLines instead of the first. Default false."},
                    "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 600000, "description": "Timeout in milliseconds. Default 120000."},
                    "allowFileWrites": {"type": "boolean", "description": "Allow redirects, tee, sed -i or Set-Content to write file contents. Default false (blocked)."}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "python".into(),
            description: "Run Python code through stdin, with no shell quoting. For data/spreadsheet processing and verification scripts. Starts in the workspace with a timeout; approved like a shell command.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "Complete Python source to execute."},
                    "cwd": {"type": "string", "description": "Optional workspace directory."},
                    "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 600000, "description": "Timeout in milliseconds. Default 120000."}
                },
                "required": ["code"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "edit".into(),
            description: "Replace one exact string in one existing file. Use edit for a single exact replacement; use apply_patch for multiple hunks, multiple files, add/delete/move, or context-anchored changes. Matching tolerates CRLF/LF, typographic punctuation, and trailing whitespace. Outside-workspace targets need approval.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Workspace-relative path, or an explicit outside path."},
                    "oldString": {"type": "string", "description": "Text to replace. It must occur exactly once unless replaceAll is true."},
                    "newString": {"type": "string", "description": "Replacement text. Use an empty string to delete."},
                    "replaceAll": {"type": "boolean", "description": "Replace every occurrence. Default false."}
                },
                "required": ["filePath", "oldString", "newString"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "apply_patch".into(),
            description: "Apply a multi-file patch with add/update/delete/move. Use this for existing-file changes, multiple hunks, multiple files, or context anchors. Format: `*** Begin Patch`, Add/Update/Delete File markers, `*** End Patch`. Update hunks start with `@@` (optional context anchor); `*** End of File` anchors EOF. Context/removed/added lines use space/-/+; Add lines use +. Optional `*** Move to: path`. Outside paths need approval.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patchText": {"type": "string", "description": "Patch text using the marker format above."}
                },
                "required": ["patchText"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "write".into(),
            description: "Create a new file. Existing files are rejected unless overwrite is true. Use apply_patch to modify an existing file.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Workspace-relative path, or an explicit outside path."},
                    "content": {"type": "string", "description": "Complete file content."},
                    "overwrite": {"type": "boolean", "description": "Replace an existing file entirely. Default false."}
                },
                "required": ["filePath", "content"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "read".into(),
            description: "Read a text file with line numbers, or an image file as a visual attachment. Explicit outside paths need approval unless auto mode is on. Use offset and limit for large text files.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Workspace-relative path, or an explicit outside path."},
                    "offset": {"type": "integer", "minimum": 1, "description": "First line to return (1-based). Default 1."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 5000, "description": "Maximum lines to return. Default 2000."}
                },
                "required": ["filePath"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "grep".into(),
            description: "Search text with a regular expression; returns path:line:text. Explicit outside paths need approval unless auto mode is on.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Rust/PCRE-style regular expression."},
                    "path": {"type": "string", "description": "Optional file or directory, workspace-relative or explicit outside path. Default workspace root."},
                    "include": {"type": "string", "description": "Optional glob limiting files, e.g. **/*.rs."},
                    "caseSensitive": {"type": "boolean", "description": "Default true."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum matching lines. Default 200."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "glob".into(),
            description: "Find files by glob pattern; build and dependency directories are skipped. Explicit outside paths need approval unless auto mode is on.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob such as **/*.rs or src/**/*.ts."},
                    "path": {"type": "string", "description": "Optional directory, workspace-relative or explicit outside path."},
                    "recursive": {"type": "boolean", "description": "Search subdirectories. Default true."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 2000, "description": "Maximum paths. Default 500."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "skill".into(),
            description: "Load a skill by name. Empty name lists skills. Without file, returns SKILL.md; with file, reads a file or lists a subfolder inside that skill only.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Skill directory name. Empty lists available skills."},
                    "file": {"type": "string", "description": "Optional path relative to the skill directory. A directory returns a listing."}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "question".into(),
            description: "Ask the user one or more questions and wait for answers. Use this when a decision or missing preference cannot be inferred. Each question can offer options; the user can also type a custom answer.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 8,
                        "items": {
                            "type": "object",
                            "properties": {
                                "header": {"type": "string", "description": "Short label for the question."},
                                "question": {"type": "string", "description": "The full question text."},
                                "multiple": {"type": "boolean", "description": "Allow selecting more than one option. Default false."},
                                "options": {
                                    "type": "array",
                                    "minItems": 1,
                                    "maxItems": 12,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {"type": "string"},
                                            "description": {"type": "string"}
                                        },
                                        "required": ["label"],
                                        "additionalProperties": false
                                    }
                                }
                            },
                            "required": ["header", "question", "options"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["questions"],
                "additionalProperties": false
            }),
        },
    ]
}

pub fn builtin_tool_def(name: &str) -> Option<ToolDef> {
    builtin_tool_defs()
        .into_iter()
        .find(|definition| definition.name == name)
}

pub fn builtin_tool_info(name: &str) -> Option<ToolInfo> {
    builtin_tool_def(name).map(|definition| ToolInfo {
        name: definition.name,
        title: None,
        description: definition.description,
        input_schema: definition.input_schema,
        output_schema: None,
        annotations: None,
    })
}

/// Returned when `write` would replace a file that already exists.
pub const WRITE_EXISTS_MESSAGE: &str = "File already exists. Use apply_patch to modify it, or pass overwrite: true to replace the whole file.";

/// The model's refusal message when a bash command writes file contents and
/// `allowFileWrites` is not set. `None` means the command may run.
fn bash_write_block(args: &Value) -> Option<&'static str> {
    let command = args.get("command").and_then(Value::as_str)?;
    let allowed = args
        .get("allowFileWrites")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (!allowed && crate::agent::shell_guard::writes_file_contents(command))
        .then_some(crate::agent::shell_guard::BLOCKED_MESSAGE)
}

pub fn is_builtin_tool(name: &str) -> bool {
    matches!(
        name,
        "read_tool_output"
            | "bash"
            | "python"
            | "edit"
            | "apply_patch"
            | "write"
            | "read"
            | "grep"
            | "glob"
            | "skill"
            | "question"
    )
}

/// Shell commands that are destructive enough to keep asking even when
/// auto-approve is on for the conversation.
pub fn is_dangerous_command(command: &str) -> bool {
    let normalized = command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }
    let destructive =
        RegexBuilder::new(r"(^|[;&|]\s*)(sudo\s+)?(rm|del|erase|rmdir|rd|unlink|shred)\b")
            .case_insensitive(true)
            .build()
            .map(|regex| regex.is_match(&normalized))
            .unwrap_or(false);
    if destructive {
        return true;
    }
    const PATTERNS: &[&str] = &[
        "format c:",
        "format /",
        "diskpart",
        "mkfs",
        "dd if=",
        "of=/dev/sd",
        "shutdown",
        "reboot",
        "poweroff",
        "git reset --hard",
        "git clean -fd",
        "git clean -f",
        "git checkout --",
        "remove-item",
        "remove-item -recurse",
        "remove-item -force",
        "invoke-expression",
        "taskkill",
        "stop-process",
        "reg delete",
        "reg add",
        "bcdedit",
        "bootrec",
        "takeown",
        "icacls",
        "chmod -r",
        "rd /s",
        "rmdir /s",
        "chown -r",
        "sc delete",
        "net user",
        "netsh ",
        "set-executionpolicy",
        "| sh",
        "| bash",
        "| powershell",
        "| iex",
        ":(){",
    ];
    PATTERNS.iter().any(|pattern| normalized.contains(pattern))
}

fn is_dangerous_tool_call(req: &ToolExecRequest) -> bool {
    if req.remote_name == "bash" {
        return req
            .args
            .get("command")
            .and_then(Value::as_str)
            .map(is_dangerous_command)
            .unwrap_or(true);
    }
    if req.remote_name == "apply_patch" {
        return req
            .args
            .get("patchText")
            .and_then(Value::as_str)
            .map(|patch| {
                patch
                    .lines()
                    .any(|line| line.starts_with("*** Delete File:"))
            })
            .unwrap_or(true);
    }
    false
}

fn auto_effective_effect(req: &ToolExecRequest, effect: Effect) -> Effect {
    if req.auto_approve && effect == Effect::Ask && !is_dangerous_tool_call(req) {
        // An explicit shell file-write escape must remain a reviewed action,
        // even when the chat is in auto mode. Otherwise the model can bypass
        // the routing guard by setting allowFileWrites itself.
        if req.remote_name == "bash"
            && req
                .args
                .get("allowFileWrites")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return Effect::Ask;
        }
        Effect::Allow
    } else {
        effect
    }
}

pub fn builtin_default_effect(name: &str) -> Effect {
    match name {
        "bash" | "python" | "edit" | "apply_patch" | "write" => Effect::Ask,
        _ => Effect::Allow,
    }
}

pub fn skill_roots(workspace: &Path, output_dir: &Path) -> Vec<PathBuf> {
    skill_roots_with_home(workspace, output_dir, home_dir().as_deref())
}

fn skill_roots_with_home(workspace: &Path, output_dir: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![
        workspace.join(".mcp-chat").join("skills"),
        workspace.join(".opencode").join("skills"),
    ];
    if let Some(parent) = output_dir.parent() {
        roots.push(parent.join("skills"));
    }
    if let Some(home) = home {
        roots.push(home.join(".config").join("opencode").join("skills"));
        roots.push(home.join(".config").join("mcp-chat").join("skills"));
    }
    roots
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn skill_description(path: &Path) -> String {
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let content = content.chars().take(8_000).collect::<String>();
    let mut frontmatter = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "---" {
            frontmatter = !frontmatter;
            continue;
        }
        if frontmatter {
            if let Some(value) = line.strip_prefix("description:") {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return truncate_chars(value, 1024).0;
                }
            }
        }
    }
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("---") {
            continue;
        }
        return truncate_chars(line, 1024).0;
    }
    String::new()
}

pub fn skill_catalog(workspace: &Path, output_dir: &Path) -> Vec<(String, String)> {
    let mut skills = BTreeMap::new();
    for root in skill_roots(workspace, output_dir) {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let manifest = path.join("SKILL.md");
            if !manifest.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            skills
                .entry(name.to_owned())
                .or_insert_with(|| skill_description(&manifest));
        }
    }
    skills.into_iter().collect()
}

impl ToolExecutor {
    pub async fn settle_error(
        &self,
        req: ToolExecRequest,
        state: &str,
        message: String,
    ) -> ToolExecResult {
        self.fail(req, state, message, Instant::now(), false).await
    }

    pub async fn execute(&self, mut req: ToolExecRequest) -> ToolExecResult {
        let started = Instant::now();
        let exposed_for_error = req.exposed_name.clone();
        if req.disabled {
            return self
                .fail(
                    req,
                    "error",
                    "Tools are disabled after the maximum steps for this turn.".into(),
                    started,
                    false,
                )
                .await;
        }

        let _permit = match self.limiter.acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                return self
                    .fail(
                        req,
                        "error",
                        "The tool runner is shutting down.".into(),
                        started,
                        false,
                    )
                    .await;
            }
        };

        let builtin = req.server_slug == "builtin";
        let tool = req.tool.clone().or_else(|| {
            builtin
                .then(|| builtin_tool_info(&req.remote_name))
                .flatten()
        });
        let Some(tool) = tool else {
            return self
                .fail(
                    req,
                    "error",
                    format!(
                        "No tool named {:?} is available. Use one from the tool list.",
                        exposed_for_error
                    ),
                    started,
                    false,
                )
                .await;
        };

        if let Err(message) = validate_args(&tool.input_schema, &req.args) {
            return self
                .fail(
                    req,
                    "error",
                    format!("Invalid tool input: {message}"),
                    started,
                    false,
                )
                .await;
        }

        if builtin && req.remote_name == MCP_EXECUTE {
            return self.execute_mcp_batch(req, started).await;
        }
        if builtin && req.remote_name == MCP_CALL_TOOL {
            return self.execute_mcp_call(req, started).await;
        }

        // Refuse a call that is certain to be rejected before asking the user
        // to approve it.
        if builtin {
            if let Some(message) = self.precheck_builtin(&req).await {
                return self.fail(req, "error", message, started, false).await;
            }
        }

        let action = if builtin {
            format!("builtin.{}", req.remote_name)
        } else {
            format!("mcp.{}.{}", req.server_slug, req.remote_name)
        };
        let resource = permissions::permission_resource(&req.args);
        let default = if builtin {
            builtin_default_effect(&req.remote_name)
        } else {
            req.default_effect
        };
        let rules = if builtin || req.server_id.is_empty() {
            rules_repo::list(&self.db.pool).await.unwrap_or_default()
        } else {
            rules_repo::list_for_server(&self.db.pool, &req.server_id)
                .await
                .unwrap_or_default()
        };
        let outside_write = if builtin {
            match req.remote_name.as_str() {
                "write" | "edit" => {
                    if let Some(raw) = req.args.get("filePath").and_then(Value::as_str) {
                        match workspace_root_for_conversation(&self.db.pool, &req.conversation_id)
                            .await
                        {
                            Ok(workspace) => path_is_outside(&workspace, raw),
                            Err(_) => false,
                        }
                    } else {
                        false
                    }
                }
                "apply_patch" => {
                    if let Some(patch_text) = req.args.get("patchText").and_then(Value::as_str) {
                        match workspace_root_for_conversation(&self.db.pool, &req.conversation_id)
                            .await
                        {
                            Ok(workspace) => {
                                crate::agent::patch::touches_outside(&workspace, patch_text)
                            }
                            Err(_) => false,
                        }
                    } else {
                        false
                    }
                }
                _ => false,
            }
        } else {
            false
        };
        let outside_read = if builtin {
            if let Some(raw) = outside_read_target(&req.remote_name, &req.args) {
                match workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await {
                    Ok(workspace) => path_is_outside(&workspace, raw),
                    Err(_) => false,
                }
            } else {
                false
            }
        } else {
            false
        };
        let outside_access = outside_write || (outside_read && !req.auto_approve);
        let evaluated = permissions::evaluate(&action, &resource, default, &rules);
        // Outside writes always need a per-call decision. Outside reads need
        // one too unless this chat is already in auto mode. Auto mode must not
        // turn writes into implicit filesystem writes.
        let effect = if outside_access {
            if evaluated == Effect::Deny {
                Effect::Deny
            } else {
                Effect::Ask
            }
        } else {
            auto_effective_effect(&req, evaluated)
        };
        match effect {
            Effect::Allow => {}
            Effect::Deny => {
                return self
                    .fail(
                        req,
                        "error",
                        "The user's permission policy denied this tool call.".into(),
                        started,
                        false,
                    )
                    .await;
            }
            Effect::Ask => {
                if let Err(message) = self
                    .ask_permission(&req, &action, &resource, !outside_access)
                    .await
                {
                    let state = if message == "Tool execution interrupted" {
                        "aborted"
                    } else {
                        "error"
                    };
                    return self.fail(req, state, message, started, false).await;
                }
                if outside_access {
                    req.allow_outside_workspace = true;
                }
            }
        }

        self.patch_part(
            &req.part_id,
            json!({"state": "running", "approval": "allowed"}),
        )
        .await;
        let _ = self.events.send(CoreEvent::Tool {
            conversation_id: req.conversation_id.clone(),
            call_id: req.call_id.clone(),
            state: "running".into(),
            progress: None,
            message: None,
            diff: None,
        });

        let outcome = if builtin {
            if req.remote_name == MCP_SEARCH_TOOLS || req.remote_name == MCP_GET_TOOL_SCHEMA {
                self.run_meta_lookup(&req).await
            } else {
                self.run_builtin(&req).await
            }
        } else {
            self.run_mcp(&req, &tool).await
        };

        self.finish(req, started, outcome).await
    }

    /// Guards that need no approval to evaluate. Outside-workspace paths are
    /// skipped here: they are checked after the separate outside-write approval.
    async fn precheck_builtin(&self, req: &ToolExecRequest) -> Option<String> {
        match req.remote_name.as_str() {
            "bash" => bash_write_block(&req.args).map(str::to_owned),
            "write" => {
                let overwrite = req
                    .args
                    .get("overwrite")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if overwrite {
                    return None;
                }
                let raw = req.args.get("filePath").and_then(Value::as_str)?;
                let workspace =
                    workspace_root_for_conversation(&self.db.pool, &req.conversation_id)
                        .await
                        .ok()?;
                let path = resolve_write_path(&workspace, raw, false).ok()?;
                let exists = tokio::fs::metadata(&path)
                    .await
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false);
                exists.then(|| WRITE_EXISTS_MESSAGE.to_owned())
            }
            _ => None,
        }
    }

    async fn finish(
        &self,
        req: ToolExecRequest,
        started: Instant,
        outcome: Result<Rendered>,
    ) -> ToolExecResult {
        let duration_ms = started.elapsed().as_millis() as i64;
        match outcome {
            Ok(Rendered {
                mut content,
                mut structured,
                ok,
            }) => {
                // The diff is UI metadata for the transcript. Move it onto the
                // tool_call part and out of `structured`, so it is neither
                // duplicated in the tool_result part nor spilled to disk. The
                // model only ever sees `content`. Only built-in file tools set
                // it; an external MCP server's own `diff` key is left alone.
                let diff = if req.server_slug == "builtin"
                    && matches!(req.remote_name.as_str(), "edit" | "apply_patch" | "write")
                {
                    take_ui_diff(&mut structured)
                } else {
                    None
                };
                let full_text = content.clone();
                let exit_code = if matches!(req.remote_name.as_str(), "bash" | "python") {
                    structured
                        .as_ref()
                        .and_then(|value| value.get("exitCode"))
                        .and_then(Value::as_i64)
                } else {
                    None
                };
                let image = structured
                    .as_ref()
                    .and_then(|value| value.get("image"))
                    .cloned();
                let mut full_result_ref = None;
                let mut truncated = false;
                let is_pager = req.remote_name == "read_tool_output";
                // A pager page is already capped at READ_PAGE_CHARS. The JSON
                // envelope can push it past that limit; truncating the envelope
                // would hide next_offset and create a new call_id on every page.
                let output_limit = if is_pager {
                    usize::MAX
                } else if matches!(req.remote_name.as_str(), "bash" | "python") {
                    SHELL_OUTPUT_LIMIT
                } else if req.remote_name == "read" {
                    READ_OUTPUT_LIMIT
                } else {
                    MODEL_OUTPUT_LIMIT
                };
                if content.chars().count() > output_limit {
                    let (cut, _) = truncate_chars(&full_text, output_limit);
                    match self.spill(&req.call_id, &full_text, &structured).await {
                        Ok(reference) => {
                            full_result_ref = Some(reference);
                            content = format!(
                                "{cut}\nFull output is available as call_id {:?}; use read_tool_output only if you need details beyond this preview.",
                                req.call_id
                            );
                        }
                        Err(_) => {
                            content = format!("{cut}\n[Full output could not be saved locally.]");
                        }
                    }
                    truncated = true;
                }
                let preview = truncate_chars(&content, PREVIEW_LIMIT).0;
                self.patch_part(
                    &req.part_id,
                    json!({
                        "state": if ok { "completed" } else { "error" },
                        "executed": true,
                        "endedAt": now_ms(),
                        "resultPreview": preview,
                        "fullResultRef": full_result_ref,
                        "truncated": truncated,
                        "exitCode": exit_code,
                        "image": image,
                        "diff": diff,
                        "error": if ok { Value::Null } else { Value::String(preview.clone()) }
                    }),
                )
                .await;
                let _ = self.events.send(CoreEvent::Tool {
                    conversation_id: req.conversation_id.clone(),
                    call_id: req.call_id.clone(),
                    state: if ok { "completed" } else { "error" }.into(),
                    progress: None,
                    message: Some(preview.clone()),
                    diff,
                });
                ToolExecResult {
                    call_id: req.call_id,
                    index: req.index,
                    exposed_name: req.exposed_name,
                    remote_name: req.remote_name,
                    server_id: req.server_id,
                    server_slug: req.server_slug,
                    server_name: req.server_name,
                    ok,
                    content,
                    structured,
                    full_result_ref,
                    truncated,
                    state: if ok { "completed" } else { "error" }.into(),
                    duration_ms,
                    executed: true,
                }
            }
            Err(error) => {
                let (state, message) = if matches!(error, CoreError::Cancelled) {
                    ("aborted", "Tool execution interrupted".to_owned())
                } else {
                    ("error", error.user_message())
                };
                self.fail(req, state, message, started, true).await
            }
        }
    }

    async fn ask_permission(
        &self,
        req: &ToolExecRequest,
        action: &str,
        resource: &str,
        allow_always: bool,
    ) -> std::result::Result<(), String> {
        self.ask_permission_explicit(
            &req.conversation_id,
            &req.part_id,
            &req.call_id,
            &req.server_id,
            &req.server_name,
            &req.remote_name,
            &req.args,
            &req.cancel,
            action,
            resource,
            allow_always,
        )
        .await
    }

    /// Build a read-only diff for built-in file mutations. Outside-workspace
    /// paths are deliberately skipped: their contents must not be read before
    /// the user grants the separate outside-write approval.
    async fn approval_diff(
        &self,
        conversation_id: &str,
        server_id: &str,
        tool_name: &str,
        args: &Value,
    ) -> Option<String> {
        if !server_id.is_empty() {
            return None;
        }
        let workspace = workspace_root_for_conversation(&self.db.pool, conversation_id)
            .await
            .ok()?;
        match tool_name {
            "edit" => {
                let raw = args.get("filePath").and_then(Value::as_str)?;
                let old = args.get("oldString").and_then(Value::as_str)?;
                let new = args.get("newString").and_then(Value::as_str)?;
                let replace_all = args
                    .get("replaceAll")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let path = resolve_write_path(&workspace, raw, false).ok()?;
                let content = read_text_file(&path).await.ok()?;
                let outcome = crate::agent::edit::apply(&content, old, new, replace_all).ok()?;
                Some(crate::agent::diff::file_diff(
                    &display_relative(&workspace, &path),
                    &content,
                    &outcome.content,
                ))
                .filter(|diff| !diff.is_empty())
            }
            "apply_patch" => {
                let patch = args.get("patchText").and_then(Value::as_str)?;
                crate::agent::patch::preview_with_scope(&workspace, patch, false)
                    .await
                    .ok()
                    .filter(|diff| !diff.is_empty())
            }
            "write" => {
                let raw = args.get("filePath").and_then(Value::as_str)?;
                let content = args.get("content").and_then(Value::as_str)?;
                let path = resolve_write_path(&workspace, raw, false).ok()?;
                let previous = if path.exists() {
                    read_text_file(&path).await.ok()?
                } else {
                    String::new()
                };
                let diff = crate::agent::diff::file_diff(
                    &display_relative(&workspace, &path),
                    &previous,
                    content,
                );
                (!diff.is_empty()).then_some(diff)
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn ask_permission_explicit(
        &self,
        conversation_id: &str,
        part_id: &str,
        call_id: &str,
        server_id: &str,
        server_name: &str,
        tool_name: &str,
        args: &Value,
        cancel: &CancellationToken,
        action: &str,
        resource: &str,
        allow_always: bool,
    ) -> std::result::Result<(), String> {
        self.patch_part(part_id, json!({"approval": "pending"}))
            .await;
        let diff = self
            .approval_diff(conversation_id, server_id, tool_name, args)
            .await;
        let _ = self.events.send(CoreEvent::Approval {
            conversation_id: conversation_id.to_owned(),
            call_id: call_id.to_owned(),
            server: server_name.to_owned(),
            tool: tool_name.to_owned(),
            args: args.clone(),
            allow_always,
            diff,
        });
        let receiver = self.approvals.register(call_id);
        let answer = tokio::select! {
            _ = cancel.cancelled() => {
                self.approvals.cancel(call_id);
                return Err("Tool execution interrupted".into());
            }
            answer = receiver => answer.ok(),
        };
        let Some(answer) = answer else {
            return Err("The approval prompt was closed without an answer.".into());
        };
        match answer.decision {
            ApprovalDecision::Once => Ok(()),
            ApprovalDecision::Always => {
                let server_id = (!server_id.is_empty()).then_some(server_id);
                let _ =
                    rules_repo::create(&self.db.pool, server_id, action, resource, "allow").await;
                Ok(())
            }
            ApprovalDecision::Reject => {
                let feedback = answer
                    .feedback
                    .unwrap_or_default()
                    .trim()
                    .chars()
                    .take(2000)
                    .collect::<String>();
                if feedback.is_empty() {
                    Err("The user declined this tool call".into())
                } else {
                    Err(format!("User rejected this call: {feedback}"))
                }
            }
        }
    }

    async fn execute_mcp_call(&self, req: ToolExecRequest, started: Instant) -> ToolExecResult {
        let server_slug = req
            .args
            .get("server")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let tool_name = req
            .args
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let target_args = req
            .args
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if server_slug.is_empty() || tool_name.is_empty() || !target_args.is_object() {
            return self
                .fail(
                    req,
                    "error",
                    "mcp_call_tool needs server, tool and an arguments object.".into(),
                    started,
                    false,
                )
                .await;
        }
        if !req
            .active_server_slugs
            .iter()
            .any(|slug| slug == &server_slug)
        {
            return self
                .fail(
                    req,
                    "error",
                    format!(
                        "MCP server {server_slug:?} is not enabled for this conversation. Use mcp_search_tools to list available servers."
                    ),
                    started,
                    false,
                )
                .await;
        }

        let (server, tool) = match self.mcp.resolve_tool(&server_slug, &tool_name).await {
            Ok(value) => value,
            Err(error) => {
                return self
                    .fail(req, "error", error.user_message(), started, false)
                    .await;
            }
        };
        if let Err(message) = validate_args(&tool.input_schema, &target_args) {
            let schema = serde_json::to_string(&tool.input_schema).unwrap_or_default();
            let schema = truncate_chars(&schema, 1500).0;
            return self
                .fail(
                    req,
                    "error",
                    format!("Invalid tool input: {message}\nSchema: {schema}"),
                    started,
                    false,
                )
                .await;
        }

        let action = format!("mcp.{}.{}", server.slug, tool.name);
        let resource = permissions::permission_resource(&target_args);
        let default = match server.policy.as_str() {
            "auto" => Effect::Allow,
            "deny" => Effect::Deny,
            _ => Effect::Ask,
        };
        let rules = rules_repo::list_for_server(&self.db.pool, &server.id)
            .await
            .unwrap_or_default();
        let effect = permissions::evaluate(&action, &resource, default, &rules);
        let effect = if req.auto_approve && effect == Effect::Ask {
            Effect::Allow
        } else {
            effect
        };
        match effect {
            Effect::Allow => {}
            Effect::Deny => {
                return self
                    .fail(
                        req,
                        "error",
                        "The user's permission policy denied this tool call.".into(),
                        started,
                        false,
                    )
                    .await;
            }
            Effect::Ask => {
                if let Err(message) = self
                    .ask_permission_explicit(
                        &req.conversation_id,
                        &req.part_id,
                        &req.call_id,
                        &server.id,
                        &server.name,
                        &tool.name,
                        &target_args,
                        &req.cancel,
                        &action,
                        &resource,
                        true,
                    )
                    .await
                {
                    let state = if message == "Tool execution interrupted" {
                        "aborted"
                    } else {
                        "error"
                    };
                    return self.fail(req, state, message, started, false).await;
                }
            }
        }

        self.patch_part(
            &req.part_id,
            json!({"state": "running", "approval": "allowed"}),
        )
        .await;
        let _ = self.events.send(CoreEvent::Tool {
            conversation_id: req.conversation_id.clone(),
            call_id: req.call_id.clone(),
            state: "running".into(),
            progress: None,
            message: None,
            diff: None,
        });

        let target_request = ToolExecRequest {
            conversation_id: req.conversation_id.clone(),
            message_id: req.message_id.clone(),
            part_id: req.part_id.clone(),
            call_id: req.call_id.clone(),
            index: req.index,
            exposed_name: req.exposed_name.clone(),
            remote_name: tool.name.clone(),
            server_id: server.id.clone(),
            server_slug: server.slug.clone(),
            server_name: server.name.clone(),
            args: target_args,
            tool: Some(tool.clone()),
            default_effect: default,
            auto_approve: req.auto_approve,
            allow_outside_workspace: false,
            disabled: false,
            cancel: req.cancel.clone(),
            active_server_slugs: req.active_server_slugs.clone(),
        };
        let outcome = self.run_mcp(&target_request, &tool).await;
        self.finish(req, started, outcome).await
    }

    async fn execute_mcp_batch(&self, req: ToolExecRequest, started: Instant) -> ToolExecResult {
        let Some(raw_calls) = req.args.get("calls").and_then(Value::as_array) else {
            return self
                .fail(
                    req,
                    "error",
                    "mcp_execute needs a calls array.".into(),
                    started,
                    false,
                )
                .await;
        };
        if raw_calls.is_empty() || raw_calls.len() > 8 {
            return self
                .fail(
                    req,
                    "error",
                    "mcp_execute accepts between 1 and 8 calls.".into(),
                    started,
                    false,
                )
                .await;
        }
        let mut calls = Vec::new();
        for (index, raw) in raw_calls.iter().enumerate() {
            let server = raw
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let tool = raw
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let arguments = raw.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let parse_error = if server.is_empty() || tool.is_empty() {
                Some("server and tool are required".to_owned())
            } else if !arguments.is_object() {
                Some("arguments must be an object".to_owned())
            } else {
                None
            };
            calls.push(BatchCall {
                index,
                server,
                tool,
                arguments,
                parse_error,
            });
        }

        self.patch_part(
            &req.part_id,
            json!({"state": "running", "approval": "allowed"}),
        )
        .await;
        let _ = self.events.send(CoreEvent::Tool {
            conversation_id: req.conversation_id.clone(),
            call_id: req.call_id.clone(),
            state: "running".into(),
            progress: None,
            message: Some(format!("Running {} MCP call(s)", calls.len())),
            diff: None,
        });

        let futures = calls
            .into_iter()
            .map(|call| self.run_batch_target(&req, call));
        let mut outcomes = futures::stream::iter(futures)
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        outcomes.sort_by_key(|outcome| outcome.index);

        let ok = outcomes.iter().all(|outcome| outcome.ok);
        let mut content = format!("Executed {} MCP call(s):", outcomes.len());
        for outcome in &outcomes {
            content.push_str(&format!(
                "\n\n## {} :: {} — {}\n{}",
                outcome.server,
                outcome.tool,
                if outcome.ok { "ok" } else { "error" },
                outcome.content
            ));
        }
        let structured = json!({
            "results": outcomes.iter().map(|outcome| json!({
                "server": outcome.server,
                "tool": outcome.tool,
                "ok": outcome.ok,
                "content": outcome.content,
            })).collect::<Vec<_>>()
        });
        self.finish(
            req,
            started,
            Ok(Rendered {
                content,
                structured: Some(structured),
                ok,
            }),
        )
        .await
    }

    async fn run_batch_target(&self, req: &ToolExecRequest, call: BatchCall) -> BatchOutcome {
        let error = |message: String| BatchOutcome {
            index: call.index,
            server: call.server.clone(),
            tool: call.tool.clone(),
            ok: false,
            content: message,
        };
        if let Some(message) = call.parse_error {
            return error(format!("Invalid call: {message}"));
        }
        if !req
            .active_server_slugs
            .iter()
            .any(|slug| slug == &call.server)
        {
            return error(format!(
                "MCP server {:?} is not enabled for this conversation.",
                call.server
            ));
        }
        let (server, tool) = match self.mcp.resolve_tool(&call.server, &call.tool).await {
            Ok(value) => value,
            Err(err) => return error(err.user_message()),
        };
        if let Err(message) = validate_args(&tool.input_schema, &call.arguments) {
            let schema = serde_json::to_string(&tool.input_schema).unwrap_or_default();
            let schema = truncate_chars(&schema, 1500).0;
            return error(format!("Invalid tool input: {message}\nSchema: {schema}"));
        }

        let action = format!("mcp.{}.{}", server.slug, tool.name);
        let resource = permissions::permission_resource(&call.arguments);
        let default = match server.policy.as_str() {
            "auto" => Effect::Allow,
            "deny" => Effect::Deny,
            _ => Effect::Ask,
        };
        let rules = rules_repo::list_for_server(&self.db.pool, &server.id)
            .await
            .unwrap_or_default();
        let effect = permissions::evaluate(&action, &resource, default, &rules);
        let effect = if req.auto_approve && effect == Effect::Ask {
            Effect::Allow
        } else {
            effect
        };
        match effect {
            Effect::Allow => {}
            Effect::Deny => {
                return error("The user's permission policy denied this tool call.".into());
            }
            Effect::Ask => {
                if let Err(message) = self
                    .ask_permission_explicit(
                        &req.conversation_id,
                        &req.part_id,
                        &format!("{}:batch:{}", req.call_id, call.index),
                        &server.id,
                        &server.name,
                        &tool.name,
                        &call.arguments,
                        &req.cancel,
                        &action,
                        &resource,
                        true,
                    )
                    .await
                {
                    return error(message);
                }
            }
        }

        let target_call_id = format!("{}:batch:{}", req.call_id, call.index);
        let target_request = ToolExecRequest {
            conversation_id: req.conversation_id.clone(),
            message_id: req.message_id.clone(),
            part_id: req.part_id.clone(),
            call_id: target_call_id,
            index: call.index,
            exposed_name: MCP_CALL_TOOL.into(),
            remote_name: tool.name.clone(),
            server_id: server.id.clone(),
            server_slug: server.slug.clone(),
            server_name: server.name.clone(),
            args: call.arguments,
            tool: Some(tool.clone()),
            default_effect: default,
            auto_approve: req.auto_approve,
            allow_outside_workspace: false,
            disabled: false,
            cancel: req.cancel.clone(),
            active_server_slugs: req.active_server_slugs.clone(),
        };
        match self.run_mcp(&target_request, &tool).await {
            Ok(Rendered {
                content,
                structured: _,
                ok,
            }) => BatchOutcome {
                index: call.index,
                server: server.slug,
                tool: tool.name,
                ok,
                content,
            },
            Err(err) => error(err.user_message()),
        }
    }

    async fn run_meta_lookup(&self, req: &ToolExecRequest) -> Result<Rendered> {
        match req.remote_name.as_str() {
            MCP_SEARCH_TOOLS => {
                let query = req
                    .args
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                let server_filter = req
                    .args
                    .get("server")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let limit = req
                    .args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(30)
                    .clamp(1, 50) as usize;
                if let Some(server) = &server_filter {
                    if !req.active_server_slugs.iter().any(|slug| slug == server) {
                        return Err(CoreError::Message(format!(
                            "MCP server {server:?} is not enabled for this conversation."
                        )));
                    }
                }
                let groups = self.mcp.tools_for_slugs(&req.active_server_slugs).await?;
                let mut matches = Vec::new();
                let mut available = Vec::new();
                for (server, tools) in groups {
                    if let Some(filter) = &server_filter {
                        if &server.slug != filter {
                            continue;
                        }
                    }
                    for tool in tools {
                        let haystack = format!(
                            "{} {} {} {}",
                            server.slug, server.name, tool.name, tool.description
                        )
                        .to_ascii_lowercase();
                        let hit = query.is_empty() || haystack.contains(&query);
                        available.push((server.slug.clone(), tool.clone()));
                        if hit {
                            matches.push((server.slug.clone(), tool));
                        }
                    }
                }
                matches.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
                available.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
                if available.is_empty() {
                    return Ok(Rendered {
                        content: "No MCP tools are available for this conversation. Check the selected servers in the Tools menu.".into(),
                        structured: None,
                        ok: true,
                    });
                }
                let no_match = matches.is_empty();
                let selected = if no_match { available } else { matches };
                let mut content = if no_match {
                    format!(
                        "No exact match for {:?}. These MCP tools are available ({}):\n",
                        query,
                        selected.len().min(limit)
                    )
                } else {
                    format!(
                        "MCP tools matching {:?} ({}):\n",
                        query,
                        selected.len().min(limit)
                    )
                };
                for (slug, tool) in selected.into_iter().take(limit) {
                    let description = truncate_chars(&tool.description, 240).0;
                    content.push_str(&format!("- {slug} :: {} — {description}\n", tool.name));
                }
                content.push_str("\nUse mcp_get_tool_schema only if you need full JSON schema details, then mcp_call_tool.");
                Ok(Rendered {
                    content,
                    structured: None,
                    ok: true,
                })
            }
            MCP_GET_TOOL_SCHEMA => {
                let server_slug = req.args.get("server").and_then(Value::as_str).unwrap_or("");
                let tool_name = req.args.get("tool").and_then(Value::as_str).unwrap_or("");
                if !req
                    .active_server_slugs
                    .iter()
                    .any(|slug| slug == server_slug)
                {
                    return Err(CoreError::Message(format!(
                        "MCP server {server_slug:?} is not enabled for this conversation."
                    )));
                }
                let (_, tool) = self.mcp.resolve_tool(server_slug, tool_name).await?;
                let content = serde_json::to_string_pretty(&tool.input_schema)?;
                Ok(Rendered {
                    content,
                    structured: Some(tool.input_schema),
                    ok: true,
                })
            }
            other => Err(CoreError::Message(format!(
                "Unknown MCP meta tool {other:?}."
            ))),
        }
    }

    async fn run_builtin(&self, req: &ToolExecRequest) -> Result<Rendered> {
        match req.remote_name.as_str() {
            "read_tool_output" => {
                let call_id = req
                    .args
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| CoreError::invalid("call_id is required"))?;
                let offset = req.args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let content = if let Some(line) = req.args.get("line").and_then(Value::as_u64) {
                    let max_lines = req
                        .args
                        .get("maxLines")
                        .and_then(Value::as_u64)
                        .unwrap_or(200) as usize;
                    read_spilled_lines(&self.output_dir, call_id, line as usize, max_lines).await?
                } else {
                    read_spilled(&self.output_dir, call_id, offset).await?
                };
                Ok(Rendered {
                    content,
                    structured: None,
                    ok: true,
                })
            }
            "bash" => self.run_bash(req).await,
            "python" => self.run_python(req).await,
            "read" => self.run_read(req).await,
            "write" => self.run_write(req).await,
            "edit" => self.run_edit(req).await,
            "apply_patch" => self.run_apply_patch(req).await,
            "grep" => self.run_grep(req).await,
            "glob" => self.run_glob(req).await,
            "skill" => self.run_skill(req).await,
            "question" => self.run_question(req).await,
            other => Err(CoreError::invalid(format!(
                "Unknown built-in tool: {other}"
            ))),
        }
    }

    async fn run_bash(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let command = req
            .args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("command is required"))?;
        if let Some(message) = bash_write_block(&req.args) {
            return Err(CoreError::invalid(message));
        }
        let timeout_ms = req
            .args
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(120_000)
            .clamp(1_000, 600_000);
        let max_lines = req
            .args
            .get("maxLines")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let tail = req
            .args
            .get("tail")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let requested_shell = req
            .args
            .get("shell")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        let shell = match requested_shell {
            "auto" => {
                if cfg!(windows) {
                    "powershell"
                } else {
                    "sh"
                }
            }
            "cmd" | "powershell" | "pwsh" | "sh" | "bash" => requested_shell,
            other => {
                return Err(CoreError::invalid(format!(
                    "Unknown shell {other:?}. Use auto, cmd, powershell, pwsh, sh or bash."
                )));
            }
        };
        if matches!(shell, "cmd" | "powershell") && !cfg!(windows) {
            return Err(CoreError::invalid(format!(
                "The {shell} shell is only available on Windows."
            )));
        }
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let cwd = match req.args.get("cwd").and_then(Value::as_str) {
            Some(raw) if !raw.trim().is_empty() => resolve_in_workspace(&workspace, raw)?,
            _ => workspace.clone(),
        };
        if !cwd.is_dir() {
            return Err(CoreError::invalid(format!(
                "cwd is not a directory: {}",
                cwd.display()
            )));
        }

        let mut cleanup: Option<PathBuf> = None;
        let mut child = match shell {
            "cmd" => {
                // A batch file avoids the cmd.exe/MSVC nested-quote problem. The
                // user command is written verbatim, one line at a time.
                let script_dir = std::env::temp_dir().join("mcp-chat-shell");
                tokio::fs::create_dir_all(&script_dir).await?;
                let script_path = script_dir.join(format!("{}.cmd", crate::new_id()));
                let script = format!("@echo off\r\nchcp 65001>nul\r\n{command}\r\n");
                tokio::fs::write(&script_path, script).await?;
                cleanup = Some(script_path.clone());
                let mut process = Command::new("cmd.exe");
                process.arg("/D").arg("/S").arg("/C").arg(&script_path);
                process
            }
            "powershell" | "pwsh" => {
                // -EncodedCommand takes UTF-16LE/base64, so neither Rust nor
                // PowerShell has to guess how nested quotes should be escaped.
                let prepared = powershell_script(command);
                let encoded = base64::engine::general_purpose::STANDARD.encode(
                    prepared
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect::<Vec<u8>>(),
                );
                let executable = if shell == "pwsh" {
                    "pwsh"
                } else {
                    "powershell.exe"
                };
                let mut process = Command::new(executable);
                process.args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-EncodedCommand",
                    &encoded,
                ]);
                process
            }
            "bash" => {
                let mut process = Command::new("bash");
                process.arg("-lc").arg(command);
                process
            }
            _ => {
                let mut process = Command::new("sh");
                process.arg("-lc").arg(command);
                process
            }
        };
        // Release builds use the Windows GUI subsystem, so there is no console
        // to inherit. Without this flag Windows opens a new terminal for every
        // shell tool call.
        #[cfg(windows)]
        child.creation_flags(CREATE_NO_WINDOW);
        child
            .current_dir(&cwd)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());

        let output = tokio::select! {
            _ = req.cancel.cancelled() => {
                if let Some(path) = cleanup {
                    let _ = tokio::fs::remove_file(path).await;
                }
                return Err(CoreError::Cancelled);
            }
            result = tokio::time::timeout(Duration::from_millis(timeout_ms), child.output()) => {
                match result {
                    Ok(Ok(output)) => output,
                    Ok(Err(error)) => {
                        if let Some(path) = cleanup {
                            let _ = tokio::fs::remove_file(path).await;
                        }
                        return Err(shell_launch_error(shell, error));
                    }
                    Err(_) => {
                        if let Some(path) = cleanup {
                            let _ = tokio::fs::remove_file(path).await;
                        }
                        return Err(CoreError::Message(format!(
                            "Command timed out after {timeout_ms} ms."
                        )));
                    }
                }
            }
        };
        if let Some(path) = cleanup {
            let _ = tokio::fs::remove_file(path).await;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr_raw = String::from_utf8_lossy(&output.stderr);
        let stderr = if matches!(shell, "powershell" | "pwsh") {
            clean_powershell_stderr(&stderr_raw)
        } else {
            stderr_raw.into_owned()
        };
        let (stdout, stdout_total, stdout_clipped) = clip_lines(&stdout, max_lines, tail);
        let (stderr, stderr_total, stderr_clipped) = clip_lines(&stderr, max_lines, tail);
        let echoed = echo_suspect(shell, command, &stdout);
        let exit_code = output.status.code();
        let exit = exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "terminated".into());
        let mut content = String::new();
        if echoed {
            content.push_str(
                "[warning] The shell echoed the command and did not execute it; this is usually a quoting problem. Change the shell or write a script file.\n",
            );
        }
        content.push_str(&format!(
            "[shell] {shell}\n[cwd] {}\n$ {command}\n[stdout]\n",
            cwd.display()
        ));
        if stdout.trim().is_empty() {
            content.push_str("(empty)\n");
        } else {
            content.push_str(stdout.trim_end());
            content.push('\n');
        }
        if stdout_clipped {
            content.push_str(&format!(
                "... ({} lines total, showing {} {max_lines})\n",
                stdout_total,
                if tail { "last" } else { "first" }
            ));
        }
        content.push_str("[stderr]\n");
        if stderr.trim().is_empty() {
            content.push_str("(empty)\n");
        } else {
            content.push_str(stderr.trim_end());
            content.push('\n');
        }
        if stderr_clipped {
            content.push_str(&format!(
                "... ({} lines total, showing {} {max_lines})\n",
                stderr_total,
                if tail { "last" } else { "first" }
            ));
        }
        content.push_str(&format!("[exit code {exit}]"));
        Ok(Rendered {
            content,
            structured: Some(json!({
                "exitCode": exit_code,
                "success": output.status.success(),
                "echoed": echoed,
            })),
            // A non-zero exit code is command output, not a tool execution
            // failure. Only quoting/echo and termination are tool failures.
            ok: exit_code.is_some() && !echoed,
        })
    }

    async fn run_python(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let code = req
            .args
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("code is required"))?;
        let timeout_ms = req
            .args
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(120_000)
            .clamp(1_000, 600_000);
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let cwd = match req.args.get("cwd").and_then(Value::as_str) {
            Some(raw) if !raw.trim().is_empty() => resolve_in_workspace(&workspace, raw)?,
            _ => workspace.clone(),
        };
        if !cwd.is_dir() {
            return Err(CoreError::invalid(format!(
                "cwd is not a directory: {}",
                cwd.display()
            )));
        }

        let executable = if cfg!(windows) { "python" } else { "python3" };
        let mut process = Command::new(executable);
        process
            .args(["-u", "-"])
            .current_dir(&cwd)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        process.creation_flags(CREATE_NO_WINDOW);

        let mut child = process.spawn().map_err(|error| {
            CoreError::Message(format!(
                "Python could not be started ({executable}). Install Python 3 or use shell=bash/powershell. {error}"
            ))
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            tokio::io::AsyncWriteExt::write_all(&mut stdin, code.as_bytes()).await?;
        }
        let output =
            match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output())
                .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => {
                    return Err(CoreError::Message(format!(
                        "Python process failed: {error}"
                    )));
                }
                Err(_) => {
                    return Err(CoreError::Message(format!(
                        "Python timed out after {timeout_ms} ms."
                    )));
                }
            };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code();
        let exit = exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "terminated".into());
        let mut content = format!("[python]\n[cwd] {}\n[stdout]\n", cwd.display());
        if stdout.trim().is_empty() {
            content.push_str("(empty)\n");
        } else {
            content.push_str(stdout.trim_end());
            content.push('\n');
        }
        content.push_str("[stderr]\n");
        if stderr.trim().is_empty() {
            content.push_str("(empty)\n");
        } else {
            content.push_str(stderr.trim_end());
            content.push('\n');
        }
        content.push_str(&format!("[exit code {exit}]"));
        Ok(Rendered {
            content,
            structured: Some(json!({
                "exitCode": exit_code,
                "success": output.status.success(),
            })),
            ok: exit_code.is_some(),
        })
    }

    async fn run_apply_patch(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let patch_text = req
            .args
            .get("patchText")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("patchText is required"))?;
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let paths =
            crate::agent::patch::target_paths(&workspace, patch_text, req.allow_outside_workspace)?;
        let _guards = self.file_locks.acquire(paths).await;
        let result = crate::agent::patch::apply_with_scope(
            &workspace,
            patch_text,
            req.allow_outside_workspace,
        )
        .await?;
        Ok(Rendered {
            content: result.render(),
            structured: Some(result.structured()),
            ok: true,
        })
    }

    async fn run_read(&self, req: &ToolExecRequest) -> Result<Rendered> {
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

    async fn run_write(&self, req: &ToolExecRequest) -> Result<Rendered> {
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

    async fn run_edit(&self, req: &ToolExecRequest) -> Result<Rendered> {
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

    async fn run_grep(&self, req: &ToolExecRequest) -> Result<Rendered> {
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

    async fn run_glob(&self, req: &ToolExecRequest) -> Result<Rendered> {
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

    async fn run_skill(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let name = req
            .args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let file = req
            .args
            .get("file")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let roots = skill_roots(&workspace, &self.output_dir);
        let catalog = skill_catalog(&workspace, &self.output_dir);
        if name.is_empty() {
            if file.is_some() {
                return Err(CoreError::invalid(
                    "A skill name is required when reading a skill file.",
                ));
            }
            let content = if catalog.is_empty() {
                "No skills found. Add <name>/SKILL.md to .mcp-chat/skills, .opencode/skills, app data, or ~/.config/opencode/skills.".into()
            } else {
                format!(
                    "Available skills:\n{}",
                    catalog
                        .iter()
                        .map(|(name, description)| if description.is_empty() {
                            format!("- {name}")
                        } else {
                            format!("- {name}: {description}")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
            return Ok(Rendered {
                content,
                structured: Some(
                    json!({"skills": catalog.iter().map(|(name, _)| name).collect::<Vec<_>>()}),
                ),
                ok: true,
            });
        }
        if name == "."
            || name == ".."
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            return Err(CoreError::invalid("Invalid skill name."));
        }
        let skill_dir = roots
            .iter()
            .map(|root| root.join(name))
            .find(|path| path.join("SKILL.md").is_file())
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "Skill {name:?} was not found. Available skills: {}",
                    if catalog.is_empty() {
                        "(none)".into()
                    } else {
                        catalog
                            .iter()
                            .map(|(name, _)| name.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ))
            })?;
        let canonical_dir = skill_dir.canonicalize()?;
        let requested = file.unwrap_or("SKILL.md");
        if requested.len() > 512 || Path::new(requested).is_absolute() {
            return Err(CoreError::invalid("Invalid skill file path."));
        }
        let target = skill_dir.join(requested);
        let target = target
            .canonicalize()
            .map_err(|_| CoreError::invalid(format!("Skill file not found: {requested}")))?;
        if !target.starts_with(&canonical_dir) {
            return Err(CoreError::invalid(
                "Skill file must stay inside the skill directory.",
            ));
        }
        let relative = target
            .strip_prefix(&canonical_dir)
            .unwrap_or(&target)
            .to_string_lossy()
            .replace('\\', "/");
        if target.is_dir() {
            let listing = skill_listing(&target, name, &relative).await?;
            return Ok(Rendered {
                content: listing,
                structured: None,
                ok: true,
            });
        }
        if !target.is_file() {
            return Err(CoreError::invalid("Skill path is not a file or directory."));
        }
        let content = read_text_file(&target).await?;
        let label = if file.is_none() {
            name.to_owned()
        } else {
            format!("{name}/{relative}")
        };
        Ok(Rendered {
            content: format!("# Skill: {label}\n\n{content}"),
            structured: None,
            ok: true,
        })
    }

    async fn run_question(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let questions = req
            .args
            .get("questions")
            .cloned()
            .ok_or_else(|| CoreError::invalid("questions is required"))?;
        let _ = self.events.send(CoreEvent::InputRequest {
            conversation_id: req.conversation_id.clone(),
            call_id: req.call_id.clone(),
            request: json!({"questions": questions}),
        });
        let receiver = self.inputs.register(&req.call_id);
        let answer = tokio::select! {
            _ = req.cancel.cancelled() => {
                self.inputs.cancel(&req.call_id);
                return Err(CoreError::Cancelled);
            }
            answer = receiver => answer.ok(),
        };
        let Some(InputAnswer {
            responses,
            cancelled,
        }) = answer
        else {
            return Err(CoreError::Message(
                "The question was closed without an answer.".into(),
            ));
        };
        if cancelled {
            return Err(CoreError::Message(
                "The user cancelled the question.".into(),
            ));
        }
        let answers = responses.get("answers").cloned().unwrap_or(responses);
        let mut content = String::from("The user answered:\n");
        if let Some(items) = answers.as_array() {
            for item in items {
                let header = item
                    .get("header")
                    .and_then(Value::as_str)
                    .unwrap_or("Question");
                let question = item.get("question").and_then(Value::as_str).unwrap_or("");
                let answer = item.get("answer").and_then(Value::as_str).unwrap_or("");
                content.push_str(&format!("- {header}: {answer}\n"));
                if !question.is_empty() {
                    content.push_str(&format!("  ({question})\n"));
                }
            }
        } else {
            content.push_str(&serde_json::to_string_pretty(&answers).unwrap_or_default());
            content.push('\n');
        }
        Ok(Rendered {
            content,
            structured: Some(answers),
            ok: true,
        })
    }

    async fn run_mcp(&self, req: &ToolExecRequest, tool: &ToolInfo) -> Result<Rendered> {
        let mut retry: Option<MrtrRetry> = None;
        for round in 0..=3 {
            let progress = self.progress_callback(&req.conversation_id, &req.call_id);
            let result = self
                .mcp
                .call_tool(
                    &req.server_id,
                    &req.remote_name,
                    &req.args,
                    Some(tool),
                    retry.take(),
                    progress,
                    &req.cancel,
                )
                .await?;
            if result.result_type() == "input_required" {
                if round >= 3 {
                    return Err(CoreError::Message(
                        "The server asked for input too many times (limit 3).".into(),
                    ));
                }
                let request = json!({
                    "inputRequests": result.input_requests.clone().unwrap_or_default(),
                    "requestState": result.request_state.clone(),
                });
                let _ = self.events.send(CoreEvent::InputRequest {
                    conversation_id: req.conversation_id.clone(),
                    call_id: req.call_id.clone(),
                    request,
                });
                let receiver = self.inputs.register(&req.call_id);
                let answer = tokio::select! {
                    _ = req.cancel.cancelled() => {
                        self.inputs.cancel(&req.call_id);
                        return Err(CoreError::Cancelled);
                    }
                    answer = receiver => answer.ok(),
                };
                let Some(InputAnswer {
                    responses,
                    cancelled,
                }) = answer
                else {
                    return Err(CoreError::Message(
                        "The input request was closed without an answer.".into(),
                    ));
                };
                if cancelled {
                    return Err(CoreError::Message(
                        "The user cancelled the server's input request.".into(),
                    ));
                }
                retry = Some(MrtrRetry {
                    input_responses: responses,
                    request_state: result.request_state,
                });
                continue;
            }
            let (content, structured, ok) = render_call_result(&result);
            return Ok(Rendered {
                content,
                structured,
                ok,
            });
        }
        Err(CoreError::Message(
            "The server kept requesting input without completing the tool call.".into(),
        ))
    }

    fn progress_callback(
        &self,
        conversation_id: &str,
        call_id: &str,
    ) -> Option<crate::mcp::client::ProgressFn> {
        let events = self.events.clone();
        let conversation_id = conversation_id.to_owned();
        let call_id = call_id.to_owned();
        Some(Arc::new(move |event: ProgressEvent| {
            let fraction = event
                .total
                .filter(|total| *total > 0.0)
                .map(|total| (event.progress / total).clamp(0.0, 1.0));
            let _ = events.send(CoreEvent::Tool {
                conversation_id: conversation_id.clone(),
                call_id: call_id.clone(),
                state: "running".into(),
                progress: fraction.or(Some(event.progress)),
                message: event.message,
                diff: None,
            });
        }))
    }

    async fn spill(
        &self,
        call_id: &str,
        content: &str,
        structured: &Option<Value>,
    ) -> Result<String> {
        tokio::fs::create_dir_all(&self.output_dir).await?;
        let path = self.output_dir.join(format!("{call_id}.json"));
        let body = json!({"content": content, "structured": structured});
        tokio::fs::write(&path, serde_json::to_vec_pretty(&body)?).await?;
        Ok(format!("tool-output/{call_id}.json"))
    }

    async fn fail(
        &self,
        req: ToolExecRequest,
        state: &str,
        message: String,
        started: Instant,
        executed: bool,
    ) -> ToolExecResult {
        let preview = truncate_chars(&message, PREVIEW_LIMIT).0;
        self.patch_part(
            &req.part_id,
            json!({
                "state": state,
                "executed": executed,
                "endedAt": now_ms(),
                "error": message,
                "resultPreview": preview,
            }),
        )
        .await;
        let _ = self.events.send(CoreEvent::Tool {
            conversation_id: req.conversation_id.clone(),
            call_id: req.call_id.clone(),
            state: state.into(),
            progress: None,
            message: Some(message.clone()),
            diff: None,
        });
        ToolExecResult {
            call_id: req.call_id,
            index: req.index,
            exposed_name: req.exposed_name,
            remote_name: req.remote_name,
            server_id: req.server_id,
            server_slug: req.server_slug,
            server_name: req.server_name,
            ok: false,
            content: message,
            structured: None,
            full_result_ref: None,
            truncated: false,
            state: state.into(),
            duration_ms: started.elapsed().as_millis() as i64,
            executed,
        }
    }

    async fn patch_part(&self, part_id: &str, patch: Value) {
        let Ok(part) = messages::get_part(&self.db.pool, part_id).await else {
            return;
        };
        let mut payload = part.payload();
        if let (Some(target), Some(source)) = (payload.as_object_mut(), patch.as_object()) {
            for (key, value) in source {
                if value.is_null() {
                    target.remove(key);
                } else {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
        let _ = messages::update_part_payload(&self.db.pool, part_id, &payload).await;
    }
}

struct Rendered {
    content: String,
    structured: Option<Value>,
    ok: bool,
}

/// Remove and return the non-empty `diff` string from a tool's structured output.
fn take_ui_diff(structured: &mut Option<Value>) -> Option<String> {
    let object = structured.as_mut()?.as_object_mut()?;
    match object.remove("diff")? {
        Value::String(diff) if !diff.is_empty() => Some(diff),
        _ => None,
    }
}

struct BatchCall {
    index: usize,
    server: String,
    tool: String,
    arguments: Value,
    parse_error: Option<String>,
}

struct BatchOutcome {
    index: usize,
    server: String,
    tool: String,
    ok: bool,
    content: String,
}

fn render_call_result(result: &crate::mcp::CallToolResult) -> (String, Option<Value>, bool) {
    let structured = result.structured_content.clone();
    let content = if let Some(structured) = &structured {
        serde_json::to_string_pretty(structured).unwrap_or_else(|_| "{}".into())
    } else {
        let mut parts = Vec::new();
        for item in &result.content {
            match item.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        parts.push(text.to_owned());
                    }
                }
                "resource" => {
                    let resource = item.get("resource").cloned().unwrap_or_default();
                    if let Some(text) = resource.get("text").and_then(Value::as_str) {
                        parts.push(text.to_owned());
                    } else {
                        parts.push(serde_json::to_string_pretty(&resource).unwrap_or_default());
                    }
                }
                "image" => parts.push(format!(
                    "[image {}]",
                    item.get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                )),
                "audio" => parts.push(format!(
                    "[audio {}]",
                    item.get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                )),
                "resource_link" => parts.push(format!(
                    "[resource link] {}",
                    item.get("uri").and_then(Value::as_str).unwrap_or("")
                )),
                other => parts.push(
                    serde_json::to_string_pretty(item).unwrap_or_else(|_| format!("[{other}]")),
                ),
            }
        }
        if parts.is_empty() {
            "{}".into()
        } else {
            parts.join("\n")
        }
    };
    (content, structured, !result.is_error)
}

fn validate_args(schema: &Value, args: &Value) -> std::result::Result<(), String> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| format!("tool schema is invalid: {error}"))?;
    let errors: Vec<String> = validator
        .iter_errors(args)
        .map(|error| {
            let path = error.instance_path().to_string();
            if path.is_empty() {
                error.to_string()
            } else {
                format!("{path}: {error}")
            }
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.into_iter().take(3).collect::<Vec<_>>().join("; "))
    }
}

fn strip_utf8_bom(value: &str) -> &str {
    value.strip_prefix('\u{feff}').unwrap_or(value)
}

/// Page formatted tool output without ending a page in the middle of a line.
/// Offsets returned by a previous page are line-aligned. A single line longer
/// than the page limit still gets a hard cut so paging always makes progress.
fn page_text(content: &str, offset: usize) -> (usize, String, usize, usize) {
    let total = content.chars().count();
    if offset >= total {
        return (total, String::new(), total, total);
    }

    let start = offset;

    let mut chunk: String = content.chars().skip(start).take(READ_PAGE_CHARS).collect();
    let mut next = start + chunk.chars().count();
    if next < total {
        if let Some(last_newline) = chunk.rfind('\n') {
            chunk.truncate(last_newline + 1);
            next = start + chunk.chars().count();
        }
    }
    (start, chunk, next, total)
}

async fn read_spilled_content(dir: &Path, call_id: &str) -> Result<String> {
    if call_id.is_empty()
        || call_id.len() > 128
        || !call_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(CoreError::invalid("invalid call_id"));
    }
    let path = dir.join(format!("{call_id}.json"));
    let raw = tokio::fs::read_to_string(&path).await.map_err(|_| {
        CoreError::Message(format!("No spilled output exists for call_id {call_id:?}."))
    })?;
    let body: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({"content": raw}));
    Ok(body
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            body.get("structured")
                .map(|value| serde_json::to_string_pretty(value).unwrap_or_default())
        })
        .unwrap_or_default())
}

pub async fn read_spilled(dir: &Path, call_id: &str, offset: usize) -> Result<String> {
    let content = read_spilled_content(dir, call_id).await?;
    let (start, chunk, next, total) = page_text(&content, offset);
    if chunk.is_empty() {
        return Ok(json!({
            "content": "",
            "offset": total,
            "next_offset": total,
            "total": total,
            "message": "Offset is at or beyond the end of this output."
        })
        .to_string());
    }
    Ok(json!({
        "content": chunk,
        "offset": start,
        "next_offset": next,
        "total": total,
        "truncated": next < total
    })
    .to_string())
}

pub async fn read_spilled_lines(
    dir: &Path,
    call_id: &str,
    start_line: usize,
    max_lines: usize,
) -> Result<String> {
    let content = read_spilled_content(dir, call_id).await?;
    let total_lines = if content.is_empty() {
        0
    } else {
        content.lines().count()
    };
    let start = start_line.max(1) - 1;
    if start >= total_lines {
        return Ok(json!({
            "content": "",
            "line": total_lines + 1,
            "next_line": total_lines + 1,
            "total_lines": total_lines,
            "message": "Line is at or beyond the end of this output."
        })
        .to_string());
    }
    let max_lines = max_lines.clamp(1, 5_000);
    let end = (start + max_lines).min(total_lines);
    let chunk = content
        .lines()
        .skip(start)
        .take(end - start)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(json!({
        "content": chunk,
        "line": start + 1,
        "next_line": if end < total_lines { end + 1 } else { total_lines },
        "total_lines": total_lines,
        "truncated": end < total_lines
    })
    .to_string())
}

/// Persist a tool result message. Results are appended in original call order,
/// never completion order.
pub async fn persist_result(
    executor: &ToolExecutor,
    conversation_id: &str,
    _assistant_message_id: &str,
    result: &ToolExecResult,
) -> Result<String> {
    let message = messages::create(&executor.db.pool, conversation_id, "tool", "completed").await?;
    let payload = PartPayload::ToolResult {
        call_id: result.call_id.clone(),
        name: result.exposed_name.clone(),
        server: result.server_name.clone(),
        ok: result.ok,
        content: result.content.clone(),
        structured: result.structured.clone(),
        full_result_ref: result.full_result_ref.clone(),
        truncated: result.truncated,
    };
    messages::append_part(&executor.db.pool, &message.id, "tool_result", &payload).await?;
    Ok(message.id)
}

pub async fn remove_spilled(dir: &Path, call_id: &str) -> Result<()> {
    if call_id.is_empty()
        || call_id.len() > 128
        || !call_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(CoreError::invalid("invalid call_id"));
    }
    let path = dir.join(format!("{call_id}.json"));
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CoreError::Io(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_paths_cannot_escape() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        assert!(resolve_in_workspace(&workspace, "src/main.rs").is_ok());
        assert!(resolve_in_workspace(&workspace, "../outside.txt").is_err());
        assert!(resolve_in_workspace(&workspace, "src/../../outside.txt").is_err());
    }

    #[test]
    fn absolute_paths_inside_the_workspace_are_accepted() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let target = workspace.join("notes").join("inside.txt");
        let resolved = resolve_in_workspace(&workspace, target.to_str().unwrap()).unwrap();
        assert_eq!(resolved, target);
    }

    #[test]
    fn builtin_tool_schema_stays_compact() {
        let chars: usize = builtin_tool_defs()
            .iter()
            .map(|tool| {
                tool.name.len()
                    + tool.description.len()
                    + serde_json::to_string(&tool.input_schema)
                        .map(|schema| schema.len())
                        .unwrap_or(0)
            })
            .sum();
        println!("builtin tool schema chars: {chars}");
        assert!(
            chars < 7_800,
            "builtin tool schema grew unexpectedly: {chars} characters"
        );
    }

    #[test]
    fn pager_pages_align_to_lines_when_possible() {
        let content = (1..=1_000)
            .map(|index| format!("{index:05} | {}\n", "x".repeat(100)))
            .collect::<String>();
        let (start, first, next, total) = page_text(&content, 0);
        assert_eq!(start, 0);
        assert!(next < total);
        assert!(first.ends_with('\n'));
        assert!(first.chars().count() <= READ_PAGE_CHARS);

        let (second_start, second, second_next, _) = page_text(&content, next);
        assert_eq!(second_start, next);
        assert!(second.ends_with('\n'));
        assert!(content.starts_with(&format!("{first}{second}")));

        let (third_start, third, _, _) = page_text(&content, second_next);
        assert_eq!(third_start, second_next);
        assert!(third.ends_with('\n'));
    }

    #[test]
    fn pager_hard_cuts_a_single_long_line_and_counts_unicode() {
        let content = "\u{e9}".repeat(READ_PAGE_CHARS + 100);
        let (start, page, next, total) = page_text(&content, 0);
        assert_eq!(start, 0);
        assert_eq!(page.chars().count(), READ_PAGE_CHARS);
        assert_eq!(next, READ_PAGE_CHARS);
        assert_eq!(total, READ_PAGE_CHARS + 100);

        let (second_start, tail, _, _) = page_text(&content, next);
        assert_eq!(second_start, next);
        assert_eq!(tail.chars().count(), 100);
    }

    #[test]
    fn read_strips_a_utf8_bom() {
        assert_eq!(strip_utf8_bom("\u{feff}L000001"), "L000001");
        assert_eq!(strip_utf8_bom("plain"), "plain");
    }

    #[test]
    fn outside_read_targets_are_recognized() {
        assert_eq!(
            outside_read_target("read", &json!({"filePath": "C:\\outside.txt"})),
            Some("C:\\outside.txt")
        );
        assert_eq!(
            outside_read_target("grep", &json!({"pattern": "x", "path": "..\\outside"})),
            Some("..\\outside")
        );
        assert_eq!(
            outside_read_target(
                "glob",
                &json!({"pattern": "**/*.txt", "path": "..\\outside"})
            ),
            Some("..\\outside")
        );
        assert_eq!(outside_read_target("read", &json!({})), None);
        assert_eq!(
            outside_read_target("write", &json!({"filePath": "x"})),
            None
        );
    }

    #[test]
    fn write_paths_are_detected_outside_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("outside.txt");
        assert!(!path_is_outside(&workspace, "inside.txt"));
        assert!(path_is_outside(&workspace, outside_file.to_str().unwrap()));
        assert!(resolve_write_path(&workspace, outside_file.to_str().unwrap(), false).is_err());
        assert_eq!(
            resolve_write_path(&workspace, outside_file.to_str().unwrap(), true).unwrap(),
            outside_file
        );
    }

    #[test]
    fn dangerous_shell_commands_are_classified() {
        assert!(is_dangerous_command("rm -rf build"));
        assert!(is_dangerous_command("Remove-Item -Recurse -Force C:\temp"));
        assert!(is_dangerous_command("git reset --hard"));
        assert!(is_dangerous_command("curl https://example.com/x | sh"));
        assert!(!is_dangerous_command("dir"));
        assert!(!is_dangerous_command("Get-ChildItem"));
        assert!(!is_dangerous_command("echo safe"));
    }

    #[test]
    fn auto_approve_keeps_dangerous_shell_calls_asking() {
        let mut safe = request("bash", json!({"command": "dir"}));
        safe.auto_approve = true;
        assert_eq!(auto_effective_effect(&safe, Effect::Ask), Effect::Allow);
        let mut escape = request(
            "bash",
            json!({"command": "echo hi > out.txt", "allowFileWrites": true}),
        );
        escape.auto_approve = true;
        assert_eq!(auto_effective_effect(&escape, Effect::Ask), Effect::Ask);
        let mut dangerous = request("bash", json!({"command": "rm -rf build"}));
        dangerous.auto_approve = true;
        assert_eq!(auto_effective_effect(&dangerous, Effect::Ask), Effect::Ask);
        let mut write = request("write", json!({"filePath": "a.txt", "content": "x"}));
        write.auto_approve = true;
        assert_eq!(auto_effective_effect(&write, Effect::Ask), Effect::Allow);
        let mut patch = request(
            "apply_patch",
            json!({"patchText": "*** Begin Patch\n*** Update File: a.txt\n@@\n-a\n+b\n*** End Patch"}),
        );
        patch.auto_approve = true;
        assert_eq!(auto_effective_effect(&patch, Effect::Ask), Effect::Allow);
        let mut delete = request(
            "apply_patch",
            json!({"patchText": "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch"}),
        );
        delete.auto_approve = true;
        assert_eq!(auto_effective_effect(&delete, Effect::Ask), Effect::Ask);
    }

    #[test]
    fn dangerous_builtins_require_approval() {
        assert_eq!(builtin_default_effect("bash"), Effect::Ask);
        assert_eq!(builtin_default_effect("edit"), Effect::Ask);
        assert_eq!(builtin_default_effect("apply_patch"), Effect::Ask);
        assert_eq!(builtin_default_effect("write"), Effect::Ask);
        assert_eq!(builtin_default_effect("read"), Effect::Allow);
        assert_eq!(builtin_default_effect("question"), Effect::Allow);
    }

    #[test]
    fn powershell_progress_clixml_is_removed() {
        let raw = "#< CLIXML\n<Objs Version=\"1.1.0.1\"><Obj S=\"progress\"><MS><AV>Preparing modules for first use.</AV></MS></Obj></Objs>\n";
        assert_eq!(clean_powershell_stderr(raw), "");
    }

    #[test]
    fn powershell_error_clixml_is_preserved_and_decoded() {
        let raw = "#< CLIXML\n<Objs Version=\"1.1.0.1\"><Obj S=\"error\"><S S=\"Error\">boom_x000D__x000A_after</S></Obj></Objs>";
        let cleaned = clean_powershell_stderr(raw);
        assert!(cleaned.contains("boom\r\nafter"));
    }

    #[test]
    fn missing_shell_has_a_clear_error() {
        let error = shell_launch_error("bash", std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(error.user_message().contains("Shell 'bash' was not found"));
    }

    #[test]
    fn powershell_wrapper_keeps_stderr_clean() {
        let wrapped = powershell_script("Get-ChildItem nofile.txt");
        assert!(wrapped.contains("$LASTEXITCODE"));
        assert!(wrapped.contains("[Console]::Error.WriteLine"));
        assert!(!wrapped.contains("Write-Error"));
        assert!(wrapped.contains("Get-ChildItem nofile.txt"));
    }

    #[test]
    fn echo_detection_is_limited_to_cmd() {
        assert!(echo_suspect("cmd", "cmd /C dir", "cmd /C dir"));
        assert!(!echo_suspect("powershell", "1+1", "1+1"));
        assert!(!echo_suspect("pwsh", "ok", "ok"));
    }

    #[test]
    fn powershell_plain_stderr_keeps_literal_escape_text() {
        assert_eq!(
            clean_powershell_stderr("literal _x000D_ marker"),
            "literal _x000D_ marker"
        );
    }

    #[test]
    fn powershell_prolog_is_filtered_from_stderr() {
        let raw = format!("{PS_PROLOG}\nreal error");
        assert_eq!(clean_powershell_stderr(&raw), "real error");
    }

    #[test]
    fn wrapped_powershell_prolog_is_filtered_from_stderr() {
        let wrapped = format!("{}\nreal error", PS_PROLOG.replace(';', ";\n"));
        assert_eq!(clean_powershell_stderr(&wrapped), "real error");
    }

    #[test]
    fn shell_echo_normalization_ignores_whitespace_and_edge_quotes() {
        assert_eq!(normalize_shell_text("  cmd   /C  dir \n"), "cmd /C dir");
        assert_eq!(normalize_shell_text("\"echo hi\""), "echo hi");
        assert!(shell_echoed("cmd /C dir", "cmd /C dir\r\n"));
        assert!(!shell_echoed("cmd /C dir", "Volume in drive C"));
    }

    #[test]
    fn line_clipping_supports_head_and_tail() {
        let text = "a\nb\nc\nd\n";
        assert_eq!(clip_lines(text, 2, false).0, "a\nb");
        assert_eq!(clip_lines(text, 2, true).0, "c\nd");
        assert_eq!(clip_lines(text, 0, true).0, text);
        assert_eq!(clip_lines(text, 2, true).1, 4);
        assert!(clip_lines(text, 2, true).2);
    }

    #[test]
    fn line_numbers_start_at_the_requested_offset() {
        let rendered = line_prefix("alpha\nbeta", 41);
        assert!(rendered.starts_with("   41 | alpha\n   42 | beta"));
    }

    #[tokio::test]
    async fn workspace_root_uses_the_saved_setting() {
        let db = Db::open_memory().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        settings::set(&db.pool, "workspace.root", &workspace.display().to_string())
            .await
            .unwrap();
        assert_eq!(workspace_root(&db.pool).await.unwrap(), workspace);
    }

    #[tokio::test]
    async fn conversation_workspace_overrides_the_global_default() {
        let db = Db::open_memory().await.unwrap();
        let global = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        settings::set(
            &db.pool,
            "workspace.root",
            &global.path().canonicalize().unwrap().display().to_string(),
        )
        .await
        .unwrap();
        let conversation = conversations::create(&db.pool, "Test", None, None)
            .await
            .unwrap();
        conversations::set_workspace(
            &db.pool,
            &conversation.id,
            Some(&local.path().canonicalize().unwrap().display().to_string()),
        )
        .await
        .unwrap();
        assert_eq!(
            workspace_root_for_conversation(&db.pool, &conversation.id)
                .await
                .unwrap(),
            local.path().canonicalize().unwrap()
        );
    }

    async fn test_executor(workspace: &Path) -> ToolExecutor {
        let db = Db::open_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, title, created_at, updated_at, archived, max_steps, tool_mode, workspace_root) VALUES (?1, 'Test', 0, 0, 0, 25, 'lazy', ?2)",
        )
        .bind("test-conversation")
        .bind(workspace.display().to_string())
        .execute(&db.pool)
        .await
        .unwrap();
        let secrets = Arc::new(crate::secrets::MemoryStore::default());
        let (events, _) = broadcast::channel(16);
        ToolExecutor {
            db: db.clone(),
            mcp: Arc::new(McpManager::new(db, secrets)),
            approvals: Arc::new(ApprovalRegistry::default()),
            inputs: Arc::new(InputRegistry::default()),
            events,
            output_dir: workspace.join("tool-output"),
            attachments_dir: workspace.join("attachments"),
            limiter: Arc::new(Semaphore::new(4)),
            file_locks: FileLocks::default(),
        }
    }

    fn request(name: &str, args: Value) -> ToolExecRequest {
        ToolExecRequest {
            conversation_id: "test-conversation".into(),
            message_id: "m".into(),
            part_id: "p".into(),
            call_id: format!("call-{name}"),
            index: 0,
            exposed_name: name.into(),
            remote_name: name.into(),
            server_id: String::new(),
            server_slug: "builtin".into(),
            server_name: "mcp-chat".into(),
            args,
            tool: builtin_tool_info(name),
            default_effect: builtin_default_effect(name),
            auto_approve: false,
            allow_outside_workspace: false,
            disabled: false,
            cancel: CancellationToken::new(),
            active_server_slugs: Vec::new(),
        }
    }

    #[tokio::test]
    async fn builtin_apply_patch_updates_workspace() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("a.txt"), "one\ntwo\n").unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_apply_patch(&request(
                "apply_patch",
                json!({
                    "patchText": "*** Begin Patch\n*** Update File: a.txt\n@@\n one\n-two\n+three\n*** Add File: b.txt\n+new\n*** End Patch\n"
                }),
            ))
            .await
            .unwrap();
        assert!(result.content.contains("M a.txt"));
        assert!(result.content.contains("A b.txt"));
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "one\nthree\n"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("b.txt")).unwrap(),
            "new\n"
        );
    }

    #[tokio::test]
    async fn builtin_file_tools_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;

        let write = executor
            .run_write(&request(
                "write",
                json!({"filePath": "notes/a.txt", "content": "hello world"}),
            ))
            .await
            .unwrap();
        assert!(write.content.contains("Wrote notes/a.txt"));

        let read = executor
            .run_read(&request("read", json!({"filePath": "notes/a.txt"})))
            .await
            .unwrap();
        assert!(read.content.contains("hello world"));
        assert!(read.content.contains("   1 |"));

        let edit = executor
            .run_edit(&request(
                "edit",
                json!({
                    "filePath": "notes/a.txt",
                    "oldString": "world",
                    "newString": "there"
                }),
            ))
            .await
            .unwrap();
        assert!(edit.content.contains("replaced 1"));
        assert_eq!(
            std::fs::read_to_string(workspace.join("notes/a.txt")).unwrap(),
            "hello there"
        );
    }

    #[tokio::test]
    async fn builtin_edit_matches_lf_text_in_a_crlf_file() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("windows.txt"), "one\r\ntwo\r\n").unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_edit(&request(
                "edit",
                json!({
                    "filePath": "windows.txt",
                    "oldString": "one\ntwo",
                    "newString": "ONE\nTWO"
                }),
            ))
            .await
            .unwrap();
        assert!(result.content.contains("replaced 1"));
        assert_eq!(
            std::fs::read_to_string(workspace.join("windows.txt")).unwrap(),
            "ONE\r\nTWO\r\n"
        );
    }

    #[tokio::test]
    async fn builtin_edit_rejects_a_noop() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("noop.txt"), "same\n").unwrap();
        let executor = test_executor(&workspace).await;
        let error = match executor
            .run_edit(&request(
                "edit",
                json!({
                    "filePath": "noop.txt",
                    "oldString": "same",
                    "newString": "same"
                }),
            ))
            .await
        {
            Ok(_) => panic!("no-op edit should fail"),
            Err(error) => error,
        };
        assert!(error.user_message().contains("identical"));
    }

    #[tokio::test]
    async fn approval_diff_shows_an_edit_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("preview.txt"), "old\n").unwrap();
        let executor = test_executor(&workspace).await;
        let diff = executor
            .approval_diff(
                "test-conversation",
                "",
                "edit",
                &json!({
                    "filePath": "preview.txt",
                    "oldString": "old",
                    "newString": "new"
                }),
            )
            .await
            .unwrap();
        assert!(diff.contains("-old"));
        assert!(diff.contains("+new"));
        assert_eq!(
            std::fs::read_to_string(workspace.join("preview.txt")).unwrap(),
            "old\n"
        );
    }

    #[tokio::test]
    async fn edit_returns_a_diff_built_before_the_file_is_written() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("preview.txt"), "keep\nold\n").unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_edit(&request(
                "edit",
                json!({
                    "filePath": "preview.txt",
                    "oldString": "old",
                    "newString": "new"
                }),
            ))
            .await
            .unwrap();
        let structured = result.structured.as_ref().unwrap();
        assert_eq!(structured["path"], "preview.txt");
        assert_eq!(structured["replacements"], 1);
        let diff = structured["diff"].as_str().unwrap();
        assert!(diff.contains("--- a/preview.txt"), "{diff}");
        assert!(diff.contains("+++ b/preview.txt"), "{diff}");
        assert!(diff.contains("-old"), "{diff}");
        assert!(diff.contains("+new"), "{diff}");
        // The file is already rewritten. A diff made from it would be empty, so
        // the removed line proves the before-text was captured before the write.
        let on_disk = std::fs::read_to_string(workspace.join("preview.txt")).unwrap();
        assert_eq!(on_disk, "keep\nnew\n");
        assert!(!on_disk.contains("old"));
        // The model-facing text carries no diff.
        assert!(!result.content.contains("@@"), "{}", result.content);
        assert!(!result.content.contains("+new"), "{}", result.content);
    }

    #[tokio::test]
    async fn write_returns_a_diff_against_the_previous_content() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("a.txt"), "keep\nold\n").unwrap();
        std::fs::write(workspace.join("blob.bin"), b"\0\x01\x02").unwrap();
        let executor = test_executor(&workspace).await;

        let overwrite = executor
            .run_write(&request(
                "write",
                json!({"filePath": "a.txt", "content": "keep\nnew\n", "overwrite": true}),
            ))
            .await
            .unwrap();
        let diff = overwrite.structured.as_ref().unwrap()["diff"]
            .as_str()
            .unwrap();
        assert!(diff.contains("-old") && diff.contains("+new"), "{diff}");

        let created = executor
            .run_write(&request(
                "write",
                json!({"filePath": "fresh.txt", "content": "hello\n"}),
            ))
            .await
            .unwrap();
        let diff = created.structured.as_ref().unwrap()["diff"]
            .as_str()
            .unwrap();
        assert!(diff.contains("+++ b/fresh.txt"), "{diff}");
        assert!(diff.contains("+hello"), "{diff}");

        // A binary file cannot be diffed as text. The write must still succeed.
        let binary = executor
            .run_write(&request(
                "write",
                json!({"filePath": "blob.bin", "content": "text\n", "overwrite": true}),
            ))
            .await
            .unwrap();
        assert!(binary.ok);
        assert!(binary.structured.as_ref().unwrap()["diff"].is_null());
        assert_eq!(
            std::fs::read_to_string(workspace.join("blob.bin")).unwrap(),
            "text\n"
        );
    }

    #[tokio::test]
    async fn write_creates_a_new_file_and_its_parent_folders() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_write(&request(
                "write",
                json!({"filePath": "deep/er/new.txt", "content": "hello\n"}),
            ))
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(
            std::fs::read_to_string(workspace.join("deep/er/new.txt")).unwrap(),
            "hello\n"
        );
        assert!(
            result.structured.as_ref().unwrap()["diff"]
                .as_str()
                .unwrap()
                .contains("+hello")
        );
    }

    #[tokio::test]
    async fn write_refuses_to_replace_an_existing_file_by_default() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("keep.txt"), "original\n").unwrap();
        let executor = test_executor(&workspace).await;

        for args in [
            json!({"filePath": "keep.txt", "content": "clobbered\n"}),
            json!({"filePath": "keep.txt", "content": "clobbered\n", "overwrite": false}),
        ] {
            let error = executor
                .run_write(&request("write", args))
                .await
                .err()
                .expect("an existing file must be rejected");
            assert_eq!(error.user_message(), WRITE_EXISTS_MESSAGE);
            assert_eq!(
                std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
                "original\n"
            );
        }
        assert_eq!(
            WRITE_EXISTS_MESSAGE,
            "File already exists. Use apply_patch to modify it, or pass overwrite: true to replace the whole file."
        );
    }

    #[tokio::test]
    async fn write_with_overwrite_replaces_the_file_and_diffs_the_old_content() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("keep.txt"), "line\nold\n").unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_write(&request(
                "write",
                json!({"filePath": "keep.txt", "content": "line\nnew\n", "overwrite": true}),
            ))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
            "line\nnew\n"
        );
        let diff = result.structured.as_ref().unwrap()["diff"]
            .as_str()
            .unwrap();
        assert!(diff.contains("-old") && diff.contains("+new"), "{diff}");
    }

    #[tokio::test]
    async fn a_rejected_overwrite_is_refused_before_any_approval_prompt() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("keep.txt"), "original\n").unwrap();
        let executor = test_executor(&workspace).await;
        let mut events = executor.events.subscribe();
        // `write` asks for approval by default, so a missing precheck would hang here.
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            executor.execute(request(
                "write",
                json!({"filePath": "keep.txt", "content": "clobbered\n"}),
            )),
        )
        .await
        .expect("the call should fail without waiting for approval");
        assert!(!result.ok);
        assert!(!result.executed);
        assert_eq!(result.content, WRITE_EXISTS_MESSAGE);
        while let Ok(event) = events.try_recv() {
            assert!(!matches!(event, CoreEvent::Approval { .. }));
        }
        assert_eq!(
            std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
            "original\n"
        );
    }

    #[tokio::test]
    async fn bash_refuses_to_write_file_contents() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        for command in [
            "echo hi > out.txt",
            "echo hi >> out.txt",
            "echo hi | tee out.txt",
            "sed -i 's/a/b/' out.txt",
            "Set-Content -Path out.txt -Value hi",
            "'hi' | Out-File out.txt",
        ] {
            let error = executor
                .run_bash(&request("bash", json!({"command": command})))
                .await
                .err()
                .unwrap_or_else(|| panic!("should have been blocked: {command}"));
            let message = error.user_message();
            assert!(
                message.starts_with("BLOCKED: bash tidak boleh mengubah isi file."),
                "{message}"
            );
            assert!(message.contains("allowFileWrites=true"));
            assert!(!workspace.join("out.txt").exists(), "{command}");
        }
    }

    #[tokio::test]
    async fn bash_still_runs_commands_that_do_not_write_file_contents() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let echo = executor
            .run_bash(&request("bash", json!({"command": "echo ok"})))
            .await
            .unwrap();
        assert!(echo.content.contains("ok"));
        // File management is not a content write.
        executor
            .run_bash(&request("bash", json!({"command": "mkdir made"})))
            .await
            .unwrap();
        assert!(workspace.join("made").is_dir());
        // The quoted redirect is text, not a redirect.
        let quoted = executor
            .run_bash(&request("bash", json!({"command": "echo \"a > b\""})))
            .await
            .unwrap();
        assert!(quoted.content.contains("a > b"));
        assert!(!workspace.join("b").exists());
    }

    #[tokio::test]
    async fn allow_file_writes_lets_a_deliberate_bash_write_through() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_bash(&request(
                "bash",
                json!({"command": "echo hi > out.txt", "allowFileWrites": true}),
            ))
            .await
            .unwrap();
        assert!(result.ok);
        assert!(workspace.join("out.txt").is_file());
        // An explicit false is the same as the default.
        assert!(
            executor
                .run_bash(&request(
                    "bash",
                    json!({"command": "echo hi > other.txt", "allowFileWrites": false}),
                ))
                .await
                .is_err()
        );
        assert!(!workspace.join("other.txt").exists());
    }

    #[tokio::test]
    async fn a_blocked_bash_command_is_refused_before_any_approval_prompt() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let mut events = executor.events.subscribe();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            executor.execute(request("bash", json!({"command": "echo hi > out.txt"}))),
        )
        .await
        .expect("the call should fail without waiting for approval");
        assert!(!result.ok);
        assert!(!result.executed);
        assert_eq!(result.content, crate::agent::shell_guard::BLOCKED_MESSAGE);
        while let Ok(event) = events.try_recv() {
            assert!(!matches!(event, CoreEvent::Approval { .. }));
        }
        assert!(!workspace.join("out.txt").exists());
    }

    #[test]
    fn tool_schemas_accept_the_new_flags_and_state_the_routing_rules() {
        let defs = builtin_tool_defs();
        let def = |name: &str| defs.iter().find(|tool| tool.name == name).unwrap();

        let write = def("write");
        assert_eq!(
            write.input_schema["properties"]["overwrite"]["type"],
            "boolean"
        );
        assert!(
            validate_args(
                &write.input_schema,
                &json!({"filePath": "a", "content": "b", "overwrite": true})
            )
            .is_ok()
        );
        assert!(
            write
                .description
                .contains("Existing files are rejected unless overwrite is true")
        );
        assert!(
            write
                .description
                .contains("Use apply_patch to modify an existing file")
        );

        let bash = def("bash");
        assert_eq!(
            bash.input_schema["properties"]["allowFileWrites"]["type"],
            "boolean"
        );
        assert!(
            validate_args(
                &bash.input_schema,
                &json!({"command": "ls", "allowFileWrites": true})
            )
            .is_ok()
        );
        assert!(
            bash.description
                .contains("Do not use bash to read or modify file contents")
        );
        assert!(
            bash.description.contains(
                "does this command change the contents of a file? If yes, use a file tool."
            )
        );

        let edit = def("edit");
        assert!(
            edit.description
                .contains("Replace one exact string in one existing file")
        );
        assert!(edit.description.contains("use apply_patch for multiple hunks, multiple files, add/delete/move, or context-anchored changes"));
        // The matching rules stay in the description.
        assert!(edit.description.contains("CRLF"));
        assert!(edit.description.contains("typographic punctuation"));
        assert!(edit.description.contains("trailing whitespace"));

        let patch = def("apply_patch");
        assert!(
            patch
                .description
                .contains("Apply a multi-file patch with add/update/delete/move")
        );
        assert!(
            patch.description.contains(
                "existing-file changes, multiple hunks, multiple files, or context anchors"
            )
        );
    }

    #[tokio::test]
    async fn apply_patch_returns_a_diff_for_every_file_and_a_summary() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(workspace.join("gone.txt"), "bye\n").unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_apply_patch(&request(
                "apply_patch",
                json!({
                    "patchText": "*** Begin Patch\n*** Update File: a.txt\n@@\n one\n-two\n+three\n*** Add File: b.txt\n+new\n*** Delete File: gone.txt\n*** End Patch\n"
                }),
            ))
            .await
            .unwrap();
        let structured = result.structured.as_ref().unwrap();
        let diff = structured["diff"].as_str().unwrap();
        for expected in [
            "--- a/a.txt",
            "-two",
            "+three",
            "+++ b/b.txt",
            "+new",
            "--- a/gone.txt",
            "-bye",
        ] {
            assert!(diff.contains(expected), "missing {expected:?} in {diff}");
        }
        let files = structured["files"].as_array().unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0]["path"], "a.txt");
        assert_eq!(files[0]["status"], "modified");
        assert_eq!(files[0]["additions"], 1);
        assert_eq!(files[0]["deletions"], 1);
        assert_eq!(files[1]["status"], "added");
        assert_eq!(files[2]["status"], "deleted");
        // The files were really changed, and the diff still shows the old text.
        assert!(!workspace.join("gone.txt").exists());
        assert!(
            !std::fs::read_to_string(workspace.join("a.txt"))
                .unwrap()
                .contains("two")
        );
        assert!(!result.content.contains("@@"), "{}", result.content);
    }

    async fn tool_call_part(executor: &ToolExecutor, name: &str) -> String {
        let message = messages::create(
            &executor.db.pool,
            "test-conversation",
            "assistant",
            "streaming",
        )
        .await
        .unwrap();
        messages::append_part(
            &executor.db.pool,
            &message.id,
            "tool_call",
            &PartPayload::ToolCall {
                call_id: format!("call-{name}"),
                name: name.into(),
                server: "mcp-chat".into(),
                args: json!({}),
                state: "running".into(),
                started_at: Some(1),
                ended_at: None,
                executed: false,
                approval: None,
                error: None,
                result_preview: None,
                full_result_ref: None,
                truncated: false,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn finish_moves_the_diff_onto_the_tool_call_and_out_of_the_result() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let mut events = executor.events.subscribe();
        let mut req = request("edit", json!({}));
        req.part_id = tool_call_part(&executor, "edit").await;
        let part_id = req.part_id.clone();
        let diff = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";

        let result = executor
            .finish(
                req,
                Instant::now(),
                Ok(Rendered {
                    content: "Edited a.txt: replaced 1 occurrence(s).".into(),
                    structured: Some(json!({
                        "path": "a.txt",
                        "replacements": 1,
                        "diff": diff
                    })),
                    ok: true,
                }),
            )
            .await;

        // What persists as the tool_result, and so what the model can see.
        assert_eq!(result.content, "Edited a.txt: replaced 1 occurrence(s).");
        let structured = result.structured.as_ref().unwrap();
        assert_eq!(structured["path"], "a.txt");
        assert!(structured.get("diff").is_none(), "{structured}");

        // The transcript renders the tool_call part, so the diff lives there.
        let stored = messages::get_part(&executor.db.pool, &part_id)
            .await
            .unwrap()
            .payload();
        assert_eq!(stored["state"], "completed");
        assert_eq!(stored["diff"], diff);

        // The live event carries it so the UI does not wait for a reload.
        let mut seen = None;
        while let Ok(event) = events.try_recv() {
            if let CoreEvent::Tool { state, diff, .. } = event {
                if state == "completed" {
                    seen = diff;
                }
            }
        }
        assert_eq!(seen.as_deref(), Some(diff));
    }

    #[tokio::test]
    async fn finish_leaves_an_external_servers_diff_field_alone() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let mut req = request("edit", json!({}));
        req.server_slug = "other".into();
        req.part_id = tool_call_part(&executor, "edit").await;
        let part_id = req.part_id.clone();

        let result = executor
            .finish(
                req,
                Instant::now(),
                Ok(Rendered {
                    content: "ok".into(),
                    structured: Some(json!({"diff": "server data"})),
                    ok: true,
                }),
            )
            .await;

        assert_eq!(result.structured.as_ref().unwrap()["diff"], "server data");
        let stored = messages::get_part(&executor.db.pool, &part_id)
            .await
            .unwrap()
            .payload();
        assert!(stored.get("diff").is_none());
    }

    #[test]
    fn take_ui_diff_ignores_missing_empty_and_non_string_values() {
        let mut none = None;
        assert_eq!(take_ui_diff(&mut none), None);
        let mut empty = Some(json!({"diff": ""}));
        assert_eq!(take_ui_diff(&mut empty), None);
        assert!(empty.unwrap().get("diff").is_none());
        let mut number = Some(json!({"diff": 3}));
        assert_eq!(take_ui_diff(&mut number), None);
        let mut ok = Some(json!({"diff": "x", "path": "p"}));
        assert_eq!(take_ui_diff(&mut ok).as_deref(), Some("x"));
        assert_eq!(ok.unwrap(), json!({"path": "p"}));
    }

    #[tokio::test]
    async fn builtin_shell_runs_in_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let command = if cfg!(windows) {
            "echo shell-ok"
        } else {
            "printf shell-ok"
        };
        let result = executor
            .run_bash(&request("bash", json!({"command": command})))
            .await
            .unwrap();
        assert!(result.content.contains("shell-ok"));
        assert!(result.content.contains("[shell]"));
        assert!(result.content.contains("[cwd]"));
        assert!(result.content.contains("[stdout]"));
        assert!(result.content.contains("[stderr]"));
        assert!(result.content.contains("[exit code 0]"));
    }

    #[tokio::test]
    async fn python_runs_multiline_code_without_shell_quoting() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_python(&request(
                "python",
                json!({
                    "code": "state = 'python-ok'\nprint(state)\nprint('unicode: ✓')\n"
                }),
            ))
            .await
            .unwrap();
        assert!(result.ok, "{}", result.content);
        assert!(result.content.contains("python-ok"));
        assert!(result.content.contains("unicode: ✓"));
        assert!(result.content.contains("[exit code 0]"));
    }

    #[tokio::test]
    async fn python_nonzero_exit_is_output_not_a_tool_failure() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_python(&request(
                "python",
                json!({"code": "import sys\nprint('bad')\nsys.exit(1)\n"}),
            ))
            .await
            .unwrap();
        assert!(result.ok, "{}", result.content);
        assert!(result.content.contains("[exit code 1]"));
        assert_eq!(result.structured.as_ref().unwrap()["exitCode"], 1);
    }

    #[tokio::test]
    async fn read_stages_images_as_visual_tool_results() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let image = workspace.join("pixel.png");
        std::fs::write(&image, b"\x89PNG\r\n\x1a\npixel-bytes").unwrap();
        let executor = test_executor(&workspace).await;
        let result = executor
            .run_read(&request(
                "read",
                json!({"filePath": image.to_string_lossy()}),
            ))
            .await
            .unwrap();
        assert!(result.ok);
        assert!(result.content.contains("Image loaded"));
        let image = result
            .structured
            .as_ref()
            .and_then(|value| value.get("image"))
            .expect("image metadata");
        assert_eq!(image["mime"], "image/png");
        let path = image["path"].as_str().unwrap();
        assert!(std::path::Path::new(path).is_file());
        assert!(path.contains("read-images"));
    }

    #[tokio::test]
    async fn shell_exit_code_is_output_not_a_tool_failure() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        let (shell, command) = if cfg!(windows) {
            ("cmd", "exit /b 1")
        } else {
            ("sh", "exit 1")
        };
        let result = executor
            .run_bash(&request(
                "bash",
                json!({"command": command, "shell": shell}),
            ))
            .await
            .unwrap();
        assert!(result.ok);
        assert!(result.content.contains("[exit code 1]"));
        assert_eq!(result.structured.as_ref().unwrap()["exitCode"], 1);
    }

    #[tokio::test]
    async fn read_tool_output_supports_line_paging() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("call.json"),
            json!({"content": "a\nb\nc\nd"}).to_string(),
        )
        .unwrap();
        let page = read_spilled_lines(dir.path(), "call", 2, 2).await.unwrap();
        let page: Value = serde_json::from_str(&page).unwrap();
        assert_eq!(page["line"], 2);
        assert_eq!(page["content"], "b\nc");
        assert_eq!(page["next_line"], 4);
        assert_eq!(page["total_lines"], 4);
        assert_eq!(page["truncated"], true);

        let last = read_spilled_lines(dir.path(), "call", 4, 2).await.unwrap();
        let last: Value = serde_json::from_str(&last).unwrap();
        assert_eq!(last["content"], "d");
        assert_eq!(last["truncated"], false);
    }

    #[tokio::test]
    async fn search_tools_honor_jarvisignore_and_skip_binary_files() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join("hidden")).unwrap();
        std::fs::create_dir_all(workspace.join("visible")).unwrap();
        std::fs::write(
            workspace.join(".jarvisignore"),
            "hidden/\n*.secret\nqa_neg_*.txt\n!qa_neg_keep.txt\n",
        )
        .unwrap();
        std::fs::write(workspace.join("hidden").join("a.txt"), "needle hidden\n").unwrap();
        std::fs::write(workspace.join("visible").join("b.txt"), "needle visible\n").unwrap();
        std::fs::write(workspace.join("root.secret"), "needle secret\n").unwrap();
        std::fs::write(workspace.join("root.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(workspace.join("qa_neg_drop.txt"), "needle drop\n").unwrap();
        std::fs::write(workspace.join("qa_neg_keep.txt"), "needle keep\n").unwrap();
        let executor = test_executor(&workspace).await;

        let result = executor
            .run_glob(&request("glob", json!({"pattern": "**/*.txt"})))
            .await
            .unwrap();
        assert!(result.content.contains("visible/b.txt"));
        assert!(!result.content.contains("hidden/a.txt"));
        assert!(result.content.contains("qa_neg_keep.txt"));
        assert!(!result.content.contains("qa_neg_drop.txt"));

        let result = executor
            .run_grep(&request("grep", json!({"pattern": "needle"})))
            .await
            .unwrap();
        assert!(result.content.contains("visible/b.txt"));
        assert!(!result.content.contains("hidden/a.txt"));
        assert!(!result.content.contains("root.bin"));
        assert!(result.content.contains("qa_neg_keep.txt"));
        assert!(!result.content.contains("qa_neg_drop.txt"));

        let result = executor
            .run_glob(&request(
                "glob",
                json!({"pattern": "*.txt", "recursive": false}),
            ))
            .await
            .unwrap();
        assert!(!result.content.contains("visible/b.txt"));
    }

    #[tokio::test]
    async fn builtin_search_tools_find_workspace_files() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let executor = test_executor(&workspace).await;
        executor
            .run_write(&request(
                "write",
                json!({"filePath": "src/main.txt", "content": "needle here"}),
            ))
            .await
            .unwrap();
        executor
            .run_write(&request(
                "write",
                json!({"filePath": "src/other.txt", "content": "nothing"}),
            ))
            .await
            .unwrap();

        let glob = executor
            .run_glob(&request("glob", json!({"pattern": "**/*.txt"})))
            .await
            .unwrap();
        assert!(glob.content.contains("src/main.txt"));
        assert!(glob.content.contains("src/other.txt"));

        let grep = executor
            .run_grep(&request("grep", json!({"pattern": "needle"})))
            .await
            .unwrap();
        assert!(grep.content.contains("src/main.txt:1:"));
        assert!(grep.content.contains("needle here"));
    }

    #[tokio::test]
    async fn builtin_skill_loads_skill_md() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let skill_dir = workspace.join(".mcp-chat/skills/demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "Use the demo workflow.").unwrap();
        let executor = test_executor(&workspace).await;

        let listed = executor
            .run_skill(&request("skill", json!({})))
            .await
            .unwrap();
        assert!(listed.content.contains("demo"));

        let loaded = executor
            .run_skill(&request("skill", json!({"name": "demo"})))
            .await
            .unwrap();
        assert!(loaded.content.contains("Use the demo workflow."));

        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        std::fs::write(skill_dir.join("references/api.md"), "reference content").unwrap();
        let reference = executor
            .run_skill(&request(
                "skill",
                json!({"name": "demo", "file": "references/api.md"}),
            ))
            .await
            .unwrap();
        assert!(reference.content.contains("reference content"));

        let listing = executor
            .run_skill(&request(
                "skill",
                json!({"name": "demo", "file": "references"}),
            ))
            .await
            .unwrap();
        assert!(listing.content.contains("api.md"));

        let escaped = executor
            .run_skill(&request(
                "skill",
                json!({"name": "demo", "file": "../outside.md"}),
            ))
            .await;
        assert!(escaped.is_err());
    }

    #[tokio::test]
    async fn outside_writes_require_explicit_approval_even_in_auto_mode() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.txt");
        let executor = Arc::new(test_executor(&workspace).await);
        let approvals = executor.approvals.clone();
        let mut events = executor.events.subscribe();

        let mut req = request(
            "write",
            json!({"filePath": target.to_string_lossy(), "content": "approved"}),
        );
        req.auto_approve = true;
        let handle = tokio::spawn(async move { executor.execute(req).await });

        let call_id = loop {
            let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("approval event timed out")
                .expect("event channel");
            if let CoreEvent::Approval {
                call_id,
                allow_always,
                ..
            } = event
            {
                assert!(!allow_always);
                break call_id;
            }
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(approvals.respond(
            &call_id,
            crate::agent::approvals::ApprovalAnswer {
                decision: ApprovalDecision::Once,
                feedback: None,
            }
        ));
        let result = handle.await.unwrap();
        assert!(result.ok, "{}", result.content);
        assert_eq!(std::fs::read_to_string(target).unwrap(), "approved");
    }

    #[test]
    fn global_skill_roots_are_discovered() {
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let output = workspace.path().join("tool-output");
        let roots = skill_roots_with_home(workspace.path(), &output, Some(home.path()));
        assert!(roots.contains(&home.path().join(".config/opencode/skills")));
        assert!(roots.contains(&home.path().join(".config/mcp-chat/skills")));
        assert!(roots.contains(&workspace.path().join(".mcp-chat/skills")));
    }

    #[tokio::test]
    async fn outside_reads_require_explicit_approval_when_auto_is_off() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("outside.txt");
        let outside_path = outside_file.to_string_lossy().to_string();
        std::fs::write(&outside_file, "outside content").unwrap();
        let executor = Arc::new(test_executor(&workspace).await);
        let approvals = executor.approvals.clone();
        let mut events = executor.events.subscribe();

        let req = request("read", json!({"filePath": outside_path}));
        let handle = tokio::spawn(async move { executor.execute(req).await });

        let (call_id, args) = loop {
            let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("approval event timed out")
                .expect("event channel");
            if let CoreEvent::Approval {
                call_id,
                args,
                allow_always,
                ..
            } = event
            {
                assert!(!allow_always);
                break (call_id, args);
            }
        };
        assert_eq!(args["filePath"].as_str(), Some(outside_path.as_str()));

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(approvals.respond(
            &call_id,
            crate::agent::approvals::ApprovalAnswer {
                decision: ApprovalDecision::Once,
                feedback: None,
            }
        ));
        let result = handle.await.unwrap();
        assert!(result.ok, "{}", result.content);
        assert!(result.content.contains("outside content"));
    }

    #[tokio::test]
    async fn read_only_tools_can_leave_workspace_in_auto_mode() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("outside.txt");
        std::fs::write(&outside_file, "outside content").unwrap();
        let executor = test_executor(&workspace).await;

        let mut allowed = request("read", json!({"filePath": outside_file.to_string_lossy()}));
        allowed.auto_approve = true;
        let result = executor.run_read(&allowed).await.unwrap();
        assert!(result.content.contains("outside content"));

        let mut grep = request(
            "grep",
            json!({"pattern": "outside", "path": outside.path().to_string_lossy()}),
        );
        grep.auto_approve = true;
        let result = executor.run_grep(&grep).await.unwrap();
        assert!(result.content.contains("outside content"));

        let mut glob = request(
            "glob",
            json!({"pattern": "**/*.txt", "path": outside.path().to_string_lossy()}),
        );
        glob.auto_approve = true;
        let result = executor.run_glob(&glob).await.unwrap();
        assert!(result.content.contains("outside.txt"));
    }
}
