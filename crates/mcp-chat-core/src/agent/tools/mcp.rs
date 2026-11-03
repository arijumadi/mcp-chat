//! MCP meta tools, batch execution and calls to remote MCP servers.

use super::{Rendered, ToolExecRequest, ToolExecResult, ToolExecutor, validate_args};
use crate::agent::approvals::InputAnswer;
use crate::agent::permissions::{self as permissions, Effect};
use crate::db::repo::permissions as rules_repo;
use crate::events::CoreEvent;
use crate::llm::ToolDef;
use crate::mcp::types::ToolInfo;
use crate::mcp::{MrtrRetry, ProgressEvent};
use crate::util::truncate_chars;
use crate::{CoreError, Result};
use futures::StreamExt;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;

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

pub(super) struct BatchCall {
    pub(super) index: usize,
    pub(super) server: String,
    pub(super) tool: String,
    pub(super) arguments: Value,
    pub(super) parse_error: Option<String>,
}

pub(super) struct BatchOutcome {
    pub(super) index: usize,
    pub(super) server: String,
    pub(super) tool: String,
    pub(super) ok: bool,
    pub(super) content: String,
}

pub(super) fn render_call_result(
    result: &crate::mcp::CallToolResult,
) -> (String, Option<Value>, bool) {
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

impl ToolExecutor {
    pub(super) async fn execute_mcp_call(
        &self,
        req: ToolExecRequest,
        started: Instant,
    ) -> ToolExecResult {
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
            exit_code: None,
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

    pub(super) async fn execute_mcp_batch(
        &self,
        req: ToolExecRequest,
        started: Instant,
    ) -> ToolExecResult {
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
            exit_code: None,
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

    pub(super) async fn run_batch_target(
        &self,
        req: &ToolExecRequest,
        call: BatchCall,
    ) -> BatchOutcome {
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

    pub(super) async fn run_meta_lookup(&self, req: &ToolExecRequest) -> Result<Rendered> {
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

    pub(super) async fn run_mcp(&self, req: &ToolExecRequest, tool: &ToolInfo) -> Result<Rendered> {
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

    pub(super) fn progress_callback(
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
                exit_code: None,
            });
        }))
    }
}
