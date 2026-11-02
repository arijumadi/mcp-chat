//! Built-in tool execution: the executor, permission flow and result handling.
//! Tool families live in the sibling modules.

mod defs;
mod files;
mod guards;
mod mcp;
mod output;
mod patch;
mod question;
mod shell;
mod skills;
#[cfg(test)]
mod tests;
mod ui;
mod workspace;

pub use defs::{
    builtin_default_effect, builtin_tool_def, builtin_tool_defs, builtin_tool_info, is_builtin_tool,
};
pub use guards::{WRITE_EXISTS_MESSAGE, is_dangerous_command};
pub use mcp::{
    MCP_CALL_TOOL, MCP_EXECUTE, MCP_GET_TOOL_SCHEMA, MCP_SEARCH_TOOLS, is_meta_tool,
    meta_tool_defs, meta_tool_info,
};
pub use output::{persist_result, read_spilled, read_spilled_lines, remove_spilled};
pub use skills::{skill_catalog, skill_roots};
pub(crate) use workspace::{display_relative, path_is_outside, resolve_write_path};
pub use workspace::{resolve_workspace_path, workspace_root, workspace_root_for_conversation};

use self::guards::auto_effective_effect;
use self::ui::take_ui_diff;
use self::workspace::outside_read_target;
use crate::agent::approvals::{ApprovalDecision, ApprovalRegistry, InputRegistry};
use crate::agent::file_locks::FileLocks;
use crate::agent::permissions::{self as permissions, Effect};
use crate::db::Db;
use crate::db::repo::{messages, permissions as rules_repo};
use crate::events::CoreEvent;
use crate::mcp::manager::McpManager;
use crate::mcp::types::ToolInfo;
use crate::util::truncate_chars;
use crate::{CoreError, Result, now_ms};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;

const MODEL_OUTPUT_LIMIT: usize = 12_000;

const SHELL_OUTPUT_LIMIT: usize = 16_000;

const READ_OUTPUT_LIMIT: usize = 24_000;

const PREVIEW_LIMIT: usize = 4_000;

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

struct Rendered {
    content: String,
    structured: Option<Value>,
    ok: bool,
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
