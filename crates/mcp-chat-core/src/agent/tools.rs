use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use crate::db::repo::{messages, permissions as rules_repo, settings};
use crate::events::CoreEvent;
use crate::llm::ToolDef;
use crate::mcp::manager::McpManager;
use crate::mcp::types::ToolInfo;
use crate::mcp::{MrtrRetry, ProgressEvent};
use crate::models::PartPayload;
use crate::util::truncate_chars;
use crate::{CoreError, Result, now_ms};

const MODEL_OUTPUT_LIMIT: usize = 8_000;
const PAGER_OUTPUT_LIMIT: usize = 100_000;
const READ_PAGE_CHARS: usize = 50_000;
const PREVIEW_LIMIT: usize = 4_000;

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
pub async fn workspace_root(pool: &SqlitePool) -> Result<PathBuf> {
    let configured = settings::get(pool, "workspace.root")
        .await?
        .filter(|value| !value.trim().is_empty());
    let raw = match configured {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir()?.join(path)
            }
        }
        None => std::env::current_dir()?,
    };
    let root = raw.canonicalize().map_err(|_| {
        CoreError::invalid(format!(
            "Workspace folder is not accessible: {}",
            raw.display()
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

fn resolve_in_workspace(workspace: &Path, raw: &str) -> Result<PathBuf> {
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
            description: "Read more of a tool result that was truncated. Use the call_id shown in the truncated result and an optional character offset.".into(),
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
            description: "Execute a shell command in the workspace. On Windows the command runs through cmd.exe /C; on macOS/Linux it runs through sh -lc. The user must approve potentially destructive commands.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to execute."},
                    "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 600000, "description": "Timeout in milliseconds. Default 120000."}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "edit".into(),
            description: "Modify an existing file by replacing an exact string. Paths are relative to the workspace and cannot escape it. The user is asked before files are changed.".into(),
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
            name: "write".into(),
            description: "Create a new file or overwrite an existing one in the workspace. Parent directories are created automatically. The user is asked before files are changed.".into(),
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
            description: "Read a UTF-8 text file from the workspace. Returns line-numbered content; use offset and limit for large files.".into(),
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
            description: "Search file contents with a regular expression. Returns path, line number and matching text. Paths are confined to the workspace.".into(),
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
            description: "Find files by glob pattern. Returns paths relative to the workspace. Common build and dependency directories are skipped.".into(),
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
            description: "Load a skill (a SKILL.md file) by name and return its content. Call with an empty name to list available skills. Skills are read from .mcp-chat/skills, .opencode/skills, and the app data skills directory.".into(),
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
            | "write"
            | "read"
            | "grep"
            | "glob"
            | "skill"
            | "question"
    )
}

pub fn builtin_default_effect(name: &str) -> Effect {
    match name {
        "bash" | "edit" | "write" => Effect::Ask,
        _ => Effect::Allow,
    }
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
        let effect = permissions::evaluate(&action, &resource, default, &rules);
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
        let workspace = workspace_root(&self.db.pool).await?;
        let mut child = if cfg!(windows) {
            let mut command_line = Command::new("cmd");
            command_line.arg("/C").arg(command);
            command_line
        } else {
            let mut command_line = Command::new("sh");
            command_line.arg("-lc").arg(command);
            command_line
        };
        child
            .current_dir(&workspace)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());
        let output = tokio::time::timeout(Duration::from_millis(timeout_ms), child.output())
            .await
            .map_err(|_| {
                CoreError::Message(format!("Command timed out after {timeout_ms} ms."))
            })??;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut content = format!("$ {command}\n");
        if !stdout.trim().is_empty() {
            content.push_str(stdout.trim_end());
            content.push('\n');
        }
        if !stderr.trim().is_empty() {
            content.push_str("stderr:\n");
            content.push_str(stderr.trim_end());
            content.push('\n');
        }
        if stdout.trim().is_empty() && stderr.trim().is_empty() {
            content.push_str("(no output)\n");
        }
        content.push_str(&format!(
            "[exit code {}]",
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "terminated".into())
        ));
        Ok(Rendered {
            content,
            structured: None,
            ok: output.status.success(),
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
        let workspace = workspace_root(&self.db.pool).await?;
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
        let workspace = workspace_root(&self.db.pool).await?;
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
        let workspace = workspace_root(&self.db.pool).await?;
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
        let workspace = workspace_root(&self.db.pool).await?;
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
        let workspace = workspace_root(&self.db.pool).await?;
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
        let workspace = workspace_root(&self.db.pool).await?;
        let mut roots = vec![
            workspace.join(".mcp-chat").join("skills"),
            workspace.join(".opencode").join("skills"),
        ];
        if let Some(parent) = self.output_dir.parent() {
            roots.push(parent.join("skills"));
        }
        let list = || -> Vec<String> {
            let mut names = BTreeSet::new();
            for root in &roots {
                let Ok(entries) = std::fs::read_dir(root) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.join("SKILL.md").is_file() {
                        if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
                            names.insert(name.to_owned());
                        }
                    }
                }
            }
            names.into_iter().collect()
        };
        if name.is_empty() {
            let names = list();
            let content = if names.is_empty() {
                "No skills found. Create .mcp-chat/skills/<name>/SKILL.md in the workspace.".into()
            } else {
                format!("Available skills:\n{}", names.join("\n"))
            };
            return Ok(Rendered {
                content,
                structured: Some(json!({"skills": names})),
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
            if list().is_empty() {
                "(none)".into()
            } else {
                list().join(", ")
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
    fn dangerous_builtins_require_approval() {
        assert_eq!(builtin_default_effect("bash"), Effect::Ask);
        assert_eq!(builtin_default_effect("edit"), Effect::Ask);
        assert_eq!(builtin_default_effect("write"), Effect::Ask);
        assert_eq!(builtin_default_effect("read"), Effect::Allow);
        assert_eq!(builtin_default_effect("question"), Effect::Allow);
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

    async fn test_executor(workspace: &Path) -> ToolExecutor {
        let db = Db::open_memory().await.unwrap();
        settings::set(&db.pool, "workspace.root", &workspace.display().to_string())
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
            conversation_id: "c".into(),
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
            disabled: false,
            cancel: CancellationToken::new(),
            active_server_slugs: Vec::new(),
        }
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
