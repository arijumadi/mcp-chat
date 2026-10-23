use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;

use crate::agent::approvals::{ApprovalDecision, ApprovalRegistry, InputAnswer, InputRegistry};
use crate::agent::permissions::{self, Effect};
use crate::db::Db;
use crate::db::repo::{messages, permissions as rules_repo};
use crate::events::CoreEvent;
use crate::llm::ToolDef;
use crate::mcp::manager::McpManager;
use crate::mcp::types::ToolInfo;
use crate::mcp::{MrtrRetry, ProgressEvent};
use crate::models::PartPayload;
use crate::util::truncate_chars;
use crate::{CoreError, Result, now_ms};

const MODEL_OUTPUT_LIMIT: usize = 8_000;
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

pub fn builtin_tool_def() -> ToolDef {
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
    }
}

pub fn builtin_tool_info() -> ToolInfo {
    let def = builtin_tool_def();
    ToolInfo {
        name: def.name,
        title: Some("Read tool output".into()),
        description: def.description,
        input_schema: def.input_schema,
        output_schema: None,
        annotations: None,
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
        let tool = req.tool.clone().or_else(|| builtin.then(builtin_tool_info));
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

        let action = if builtin {
            format!("builtin.{}", req.remote_name)
        } else {
            format!("mcp.{}.{}", req.server_slug, req.remote_name)
        };
        let resource = permissions::permission_resource(&req.args);
        let default = if builtin {
            Effect::Allow
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
            self.run_builtin(&req).await
        } else {
            self.run_mcp(&req, &tool).await
        };

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
                if content.chars().count() > MODEL_OUTPUT_LIMIT {
                    let (cut, _) = truncate_chars(&full_text, MODEL_OUTPUT_LIMIT);
                    match self.spill(&req.call_id, &full_text, &structured).await {
                        Ok(reference) => {
                            full_result_ref = Some(reference);
                            content = format!(
                                "{cut}\nFull output is available as call_id {:?}; use read_tool_output to page it.",
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
        self.patch_part(&req.part_id, json!({"approval": "pending"}))
            .await;
        let _ = self.events.send(CoreEvent::Approval {
            conversation_id: req.conversation_id.clone(),
            call_id: req.call_id.clone(),
            server: req.server_name.clone(),
            tool: req.remote_name.clone(),
            args: req.args.clone(),
        });
        let receiver = self.approvals.register(&req.call_id);
        let answer = tokio::select! {
            _ = req.cancel.cancelled() => {
                self.approvals.cancel(&req.call_id);
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
                let server_id = (!req.server_id.is_empty()).then_some(req.server_id.as_str());
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
    let chunk: String = content.chars().skip(offset).take(32_000).collect();
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
