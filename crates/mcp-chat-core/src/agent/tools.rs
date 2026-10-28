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
use tokio::process::Command;
use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;

use crate::agent::approvals::{ApprovalDecision, ApprovalRegistry, InputAnswer, InputRegistry};
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

const MODEL_OUTPUT_LIMIT: usize = 8_000;
const SHELL_OUTPUT_LIMIT: usize = 12_000;
const PS_PROLOG: &str = "$ProgressPreference='SilentlyContinue';[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);$OutputEncoding=[Console]::OutputEncoding;";
const PAGER_OUTPUT_LIMIT: usize = 100_000;
const READ_PAGE_CHARS: usize = 50_000;
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
    pub limiter: Arc<Semaphore>,
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
    Ok(root)
}

pub async fn workspace_root(pool: &SqlitePool) -> Result<PathBuf> {
    let configured = settings::get(pool, "workspace.root")
        .await?
        .filter(|value| !value.trim().is_empty());
    match configured {
        Some(value) => resolve_workspace_path(&value),
        None => Ok(std::env::current_dir()?.canonicalize()?),
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
    let normalized = normalize_lexically(&joined);
    if !normalized.starts_with(workspace) {
        return Err(CoreError::invalid(
            "Path is outside the selected workspace folder.",
        ));
    }
    if normalized.exists() {
        let canonical = normalized.canonicalize()?;
        if !canonical.starts_with(workspace) {
            return Err(CoreError::invalid(
                "Path resolves outside the selected workspace folder.",
            ));
        }
        return Ok(canonical);
    }
    let mut ancestor = normalized.parent();
    while let Some(path) = ancestor {
        if path.exists() {
            let canonical = path.canonicalize()?;
            if !canonical.starts_with(workspace) {
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

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn skip_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    !matches!(
        name.as_ref(),
        ".git" | "node_modules" | "target" | "dist" | "build" | ".venv" | "venv" | ".next"
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

fn shell_echoed(command: &str, stdout: &str) -> bool {
    !stdout.trim().is_empty() && normalize_shell_text(stdout) == normalize_shell_text(command)
}

/// Only cmd.exe is allowed to trigger the echo heuristic. PowerShell's
/// -EncodedCommand does not echo the script, and a command such as `$ "1+1"`
/// legitimately produces output identical to its command text.
fn echo_suspect(shell: &str, command: &str, stdout: &str) -> bool {
    shell == "cmd" && shell_echoed(command, stdout)
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
            description: "Read a truncated tool result by call_id, optionally from a character offset.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "call_id": {"type": "string", "description": "The tool call id whose spilled output should be read."},
                    "offset": {"type": "integer", "minimum": 0, "description": "Character offset to start from. Default 0."}
                },
                "required": ["call_id"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "bash".into(),
            description: "Run a shell command in the workspace. Each call is a fresh process: cd and variables do not persist. Defaults to cmd.exe on Windows and sh elsewhere. Use shell=powershell/pwsh for nested Windows quoting, or write a script for long commands. Destructive commands require approval.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to execute."},
                    "shell": {
                        "type": "string",
                        "enum": ["auto", "cmd", "powershell", "pwsh", "sh", "bash"],
                        "description": "Shell to use. Default auto: cmd.exe on Windows, sh elsewhere. PowerShell commands are passed with -EncodedCommand, so nested quotes are safe."
                    },
                    "cwd": {"type": "string", "description": "Optional working directory relative to the workspace. The process starts here; cd inside the command does not persist."},
                    "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 600000, "description": "Timeout in milliseconds. Default 120000."}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "edit".into(),
            description: "Replace an exact string in an existing workspace file. oldString must match once unless replaceAll is true. Ask before writing.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "File path relative to the workspace."},
                    "oldString": {"type": "string", "description": "Exact text to replace. It must occur exactly once unless replaceAll is true."},
                    "newString": {"type": "string", "description": "Replacement text. Use an empty string to delete."},
                    "replaceAll": {"type": "boolean", "description": "Replace every occurrence. Default false."}
                },
                "required": ["filePath", "oldString", "newString"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "apply_patch".into(),
            description: "Apply a multi-file text patch. Format: `*** Begin Patch`, then `*** Add File: path`, `*** Update File: path`, `*** Delete File: path`, and `*** End Patch`. Update hunks start with `@@`; context/removed/added lines start with space/-/+. Add File lines start with +. Optional `*** Move to: path` inside an update. Paths are workspace-relative. Ask before writing.".into(),
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
            description: "Create or overwrite a workspace file. Parent folders are created automatically. Ask before writing.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "File path relative to the workspace."},
                    "content": {"type": "string", "description": "Complete file content."}
                },
                "required": ["filePath", "content"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "read".into(),
            description: "Read a UTF-8 workspace file with line numbers. Use offset and limit for large files.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "File path relative to the workspace."},
                    "offset": {"type": "integer", "minimum": 1, "description": "First line to return (1-based). Default 1."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 5000, "description": "Maximum lines to return. Default 2000."}
                },
                "required": ["filePath"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "grep".into(),
            description: "Search workspace files with a regular expression; returns path:line:text.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Rust/PCRE-style regular expression."},
                    "path": {"type": "string", "description": "Optional file or directory relative to the workspace. Default workspace root."},
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
            description: "Find workspace files by glob pattern; build and dependency directories are skipped.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob such as **/*.rs or src/**/*.ts."},
                    "path": {"type": "string", "description": "Optional directory relative to the workspace."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 2000, "description": "Maximum paths. Default 500."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "skill".into(),
            description: "Load a skill (SKILL.md) by name. Empty name lists skills from .mcp-chat/skills, .opencode/skills, and app data.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Skill directory name. Empty lists available skills."}
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

pub fn is_builtin_tool(name: &str) -> bool {
    matches!(
        name,
        "read_tool_output"
            | "bash"
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
        Effect::Allow
    } else {
        effect
    }
}

pub fn builtin_default_effect(name: &str) -> Effect {
    match name {
        "bash" | "edit" | "apply_patch" | "write" => Effect::Ask,
        _ => Effect::Allow,
    }
}

pub fn skill_roots(workspace: &Path, output_dir: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        workspace.join(".mcp-chat").join("skills"),
        workspace.join(".opencode").join("skills"),
    ];
    if let Some(parent) = output_dir.parent() {
        roots.push(parent.join("skills"));
    }
    roots
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
                    return truncate_chars(value, 200).0;
                }
            }
        }
    }
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("---") {
            continue;
        }
        return truncate_chars(line, 200).0;
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

    pub async fn execute(&self, req: ToolExecRequest) -> ToolExecResult {
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
        let effect = auto_effective_effect(
            &req,
            permissions::evaluate(&action, &resource, default, &rules),
        );
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
                if let Err(message) = self.ask_permission(&req, &action, &resource).await {
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
                structured,
                ok,
            }) => {
                let full_text = content.clone();
                let mut full_result_ref = None;
                let mut truncated = false;
                let is_pager = req.remote_name == "read_tool_output";
                let output_limit = if is_pager {
                    PAGER_OUTPUT_LIMIT
                } else if req.remote_name == "bash" {
                    SHELL_OUTPUT_LIMIT
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
        )
        .await
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
    ) -> std::result::Result<(), String> {
        self.patch_part(part_id, json!({"approval": "pending"}))
            .await;
        let _ = self.events.send(CoreEvent::Approval {
            conversation_id: conversation_id.to_owned(),
            call_id: call_id.to_owned(),
            server: server_name.to_owned(),
            tool: tool_name.to_owned(),
            args: args.clone(),
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
                let content = read_spilled(&self.output_dir, call_id, offset).await?;
                Ok(Rendered {
                    content,
                    structured: None,
                    ok: true,
                })
            }
            "bash" => self.run_bash(req).await,
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
        let timeout_ms = req
            .args
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(120_000)
            .clamp(1_000, 600_000);
        let requested_shell = req
            .args
            .get("shell")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        let shell = match requested_shell {
            "auto" => {
                if cfg!(windows) {
                    "cmd"
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
                let prepared = format!("{PS_PROLOG}\n{command}");
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

        let output =
            match tokio::time::timeout(Duration::from_millis(timeout_ms), child.output()).await {
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
        let echoed = echo_suspect(shell, command, &stdout);
        let exit = output
            .status
            .code()
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
            structured: None,
            ok: output.status.success() && !echoed,
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
        let result = crate::agent::patch::apply(&workspace, patch_text).await?;
        Ok(Rendered {
            content: result.render(),
            structured: None,
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
        let path = resolve_in_workspace(&workspace, raw)?;
        let content = read_text_file(&path).await?;
        let lines = content.lines().collect::<Vec<_>>();
        let total = lines.len();
        let relative = display_relative(&workspace, &path);
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
        if content.len() as u64 > MAX_TEXT_BYTES {
            return Err(CoreError::invalid("File content is too large."));
        }
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let path = resolve_in_workspace(&workspace, raw)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        let relative = display_relative(&workspace, &path);
        Ok(Rendered {
            content: format!("Wrote {relative} ({} bytes).", content.len()),
            structured: Some(json!({"path": relative, "bytes": content.len()})),
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
        let path = resolve_in_workspace(&workspace, raw)?;
        let content = read_text_file(&path).await?;
        let count = content.matches(old).count();
        if count == 0 {
            return Err(CoreError::invalid(format!(
                "oldString was not found in {}.",
                display_relative(&workspace, &path)
            )));
        }
        if count > 1 && !replace_all {
            return Err(CoreError::invalid(format!(
                "oldString occurs {count} times in {}; set replaceAll to true or use a more specific string.",
                display_relative(&workspace, &path)
            )));
        }
        let updated = if replace_all {
            content.replace(old, new)
        } else {
            content.replacen(old, new, 1)
        };
        if updated.len() as u64 > MAX_TEXT_BYTES {
            return Err(CoreError::invalid("Edited file is too large."));
        }
        tokio::fs::write(&path, updated).await?;
        let relative = display_relative(&workspace, &path);
        Ok(Rendered {
            content: format!("Edited {relative}: replaced {count} occurrence(s)."),
            structured: Some(json!({"path": relative, "replacements": count})),
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
        let search = resolve_in_workspace(
            &workspace,
            req.args.get("path").and_then(Value::as_str).unwrap_or("."),
        )?;
        let include = req
            .args
            .get("include")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(glob_matcher)
            .transpose()?;
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|error| CoreError::invalid(format!("Invalid regular expression: {error}")))?;
        let root = workspace.clone();
        let found = tokio::task::spawn_blocking(move || -> Result<String> {
            let mut hits = Vec::new();
            let mut truncated = false;
            'files: for entry in WalkDir::new(&search)
                .follow_links(false)
                .into_iter()
                .filter_entry(skip_dir)
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
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let search = resolve_in_workspace(
            &workspace,
            req.args.get("path").and_then(Value::as_str).unwrap_or("."),
        )?;
        let matcher = glob_matcher(&pattern)?;
        let root = workspace.clone();
        let found = tokio::task::spawn_blocking(move || -> Result<String> {
            let mut paths = Vec::new();
            'walk: for entry in WalkDir::new(&search)
                .follow_links(false)
                .into_iter()
                .filter_entry(skip_dir)
            {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative = display_relative(&root, entry.path());
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
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let roots = skill_roots(&workspace, &self.output_dir);
        let catalog = skill_catalog(&workspace, &self.output_dir);
        if name.is_empty() {
            let content = if catalog.is_empty() {
                "No skills found. Create .mcp-chat/skills/<name>/SKILL.md in the workspace.".into()
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
        for root in &roots {
            let path = root.join(name).join("SKILL.md");
            if path.is_file() {
                let content = read_text_file(&path).await?;
                return Ok(Rendered {
                    content: format!("# Skill: {name}\n\n{content}"),
                    structured: None,
                    ok: true,
                });
            }
        }
        Err(CoreError::invalid(format!(
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
        )))
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

pub async fn read_spilled(dir: &Path, call_id: &str, offset: usize) -> Result<String> {
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
    let content = body
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            body.get("structured")
                .map(|value| serde_json::to_string_pretty(value).unwrap_or_default())
        })
        .unwrap_or_default();
    let total = content.chars().count();
    if offset >= total {
        return Ok(json!({
            "content": "",
            "offset": offset,
            "next_offset": total,
            "total": total,
            "message": "Offset is at or beyond the end of this output."
        })
        .to_string());
    }
    let chunk: String = content.chars().skip(offset).take(READ_PAGE_CHARS).collect();
    let next = offset + chunk.chars().count();
    Ok(json!({
        "content": chunk,
        "offset": offset,
        "next_offset": next,
        "total": total,
        "truncated": next < total
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
            chars < 6_000,
            "builtin tool schema grew unexpectedly: {chars} characters"
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
        let wrapped = "$ProgressPreference='SilentlyContinue';[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);$OutputEncodin\ng=[Console]::OutputEncoding;\nreal error";
        assert_eq!(clean_powershell_stderr(wrapped), "real error");
    }

    #[test]
    fn shell_echo_normalization_ignores_whitespace_and_edge_quotes() {
        assert_eq!(normalize_shell_text("  cmd   /C  dir \n"), "cmd /C dir");
        assert_eq!(normalize_shell_text("\"echo hi\""), "echo hi");
        assert!(shell_echoed("cmd /C dir", "cmd /C dir\r\n"));
        assert!(!shell_echoed("cmd /C dir", "Volume in drive C"));
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
            limiter: Arc::new(Semaphore::new(4)),
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
    }
}
