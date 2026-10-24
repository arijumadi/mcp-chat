use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::agent::ChatEngine;
use crate::agent::context::{build_messages, estimate_tokens, transcript_for_summary};
use crate::agent::permissions::Effect;
use crate::agent::tools::{
    MCP_SEARCH_TOOLS, ToolExecRequest, builtin_default_effect, builtin_tool_defs,
    builtin_tool_info, is_builtin_tool, is_meta_tool, meta_tool_defs, meta_tool_info,
    persist_result,
};
use crate::agent::types::{
    CONTINUE_PROMPT, LOOP_GUARD_PROMPT, MAX_STEPS_PROMPT, OUTPUT_LIMIT_PROMPT, StepError,
    is_length, retry_delay,
};
use crate::db::repo::{conversations, messages, settings};
use crate::events::{CoreEvent, TurnStatus};
use crate::llm::{
    self, ChatMessage, ChatRequest, ChatRole, ContentBlock, Delta, FinishReason, ToolChoice,
    ToolDef,
};
use crate::mcp::tools::ToolRegistry;
use crate::models::{PartPayload, ProviderKind};
use crate::{CoreError, Result, now_ms};

struct Resolved {
    kind: ProviderKind,
    base_url: String,
    api_key: Option<String>,
    model: String,
    context_window: i64,
    max_tokens: i64,
    system: Option<String>,
}

#[derive(Default)]
struct CallState {
    index: usize,
    id: String,
    name: String,
    fragment: String,
    args: Option<Value>,
    part_id: Option<String>,
    dispatched: bool,
    argued: bool,
}

struct StreamRun {
    message_id: Option<String>,
    finish: Option<FinishReason>,
    error: Option<CoreError>,
    cancelled: bool,
    had_output: bool,
    had_tools: bool,
    tool_signatures: Vec<u64>,
}

fn tool_signature(name: &str, args: &Value, ok: bool, content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    serde_json::to_string(args)
        .unwrap_or_default()
        .hash(&mut hasher);
    ok.hash(&mut hasher);
    // Spill hints embed a per-call id; ignore that unstable tail so repeated
    // large outputs still count as identical.
    content
        .split("Full output is available as call_id")
        .next()
        .unwrap_or(content)
        .hash(&mut hasher);
    hasher.finish()
}

struct PartWriter {
    kind: &'static str,
    part_id: Option<String>,
    text: String,
    dirty: bool,
    last_flush: Instant,
}

impl PartWriter {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            part_id: None,
            text: String::new(),
            dirty: false,
            last_flush: Instant::now(),
        }
    }

    async fn push(
        &mut self,
        engine: &ChatEngine,
        conversation_id: &str,
        message_id: &str,
        delta: &str,
    ) -> Result<()> {
        if self.part_id.is_none() {
            let part = messages::append_part(
                &engine.db.pool,
                message_id,
                self.kind,
                &if self.kind == "reasoning" {
                    PartPayload::Reasoning {
                        text: String::new(),
                    }
                } else {
                    PartPayload::Text {
                        text: String::new(),
                    }
                },
            )
            .await?;
            self.part_id = Some(part.id);
        }
        self.text.push_str(delta);
        self.dirty = true;
        let part_id = self.part_id.clone().unwrap_or_default();
        let _ = engine.events.send(CoreEvent::Delta {
            conversation_id: conversation_id.to_owned(),
            message_id: message_id.to_owned(),
            part_id,
            delta_type: self.kind.to_owned(),
            text: delta.to_owned(),
        });
        if self.last_flush.elapsed() > Duration::from_millis(250) {
            self.flush(engine).await?;
        }
        Ok(())
    }

    async fn flush(&mut self, engine: &ChatEngine) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(part_id) = &self.part_id {
            messages::update_part_text(&engine.db.pool, part_id, &self.text).await?;
        }
        self.dirty = false;
        self.last_flush = Instant::now();
        Ok(())
    }
}

pub async fn run(
    engine: Arc<ChatEngine>,
    conversation_id: &str,
    cancel: CancellationToken,
) -> Result<()> {
    let _ = engine.events.send(CoreEvent::Status {
        conversation_id: conversation_id.to_owned(),
        status: TurnStatus::Thinking,
        message: None,
    });
    match drive(engine.clone(), conversation_id, cancel).await {
        Ok(()) => {
            let _ = engine.events.send(CoreEvent::Status {
                conversation_id: conversation_id.to_owned(),
                status: TurnStatus::Idle,
                message: None,
            });
            Ok(())
        }
        Err(CoreError::Cancelled) => {
            let _ = engine.events.send(CoreEvent::Status {
                conversation_id: conversation_id.to_owned(),
                status: TurnStatus::Idle,
                message: Some("Stopped.".into()),
            });
            Ok(())
        }
        Err(error) => {
            fail_turn(engine.clone(), conversation_id, None, &error).await;
            Ok(())
        }
    }
}

async fn resolve_system_prompt(
    engine: &ChatEngine,
    conversation: &crate::models::Conversation,
) -> Result<Option<String>> {
    let prompt = match conversation.system_prompt.clone() {
        Some(prompt) => Some(prompt),
        None => settings::get(&engine.db.pool, "default_system_prompt")
            .await?
            .or_else(|| Some(crate::DEFAULT_SYSTEM_PROMPT.to_owned())),
    };
    Ok(prompt.filter(|value| !value.trim().is_empty()))
}

async fn drive(
    engine: Arc<ChatEngine>,
    conversation_id: &str,
    cancel: CancellationToken,
) -> Result<()> {
    let conversation = conversations::get(&engine.db.pool, conversation_id).await?;
    let (provider, model, api_key, context_window, max_tokens) =
        engine.resolve(&conversation).await?;
    let kind = ProviderKind::parse(&provider.kind)
        .ok_or_else(|| CoreError::Message(format!("Unknown provider kind {:?}", provider.kind)))?;
    let system = resolve_system_prompt(&engine, &conversation).await?;
    let resolved = Resolved {
        kind,
        base_url: provider.base_url.clone(),
        api_key,
        model,
        context_window,
        max_tokens,
        system,
    };

    let (registry, defaults, tool_defs, active_server_slugs, tool_mode) =
        load_tools(&engine, &conversation).await;
    let mut tool_choice = ToolChoice::Auto;
    let mut tools_disabled = false;
    let mut max_notice_sent = false;
    let mut final_step = false;
    let max_steps = conversation.max_steps.max(1);

    let mut step = 1_i64;
    let mut retries = 0_usize;
    let mut recover_used = false;
    let mut length_count = 0_usize;
    let mut compacted_once = false;
    let mut tool_signature_counts: HashMap<u64, usize> = HashMap::new();
    let mut loop_guard_fired = false;

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        if step > max_steps && !max_notice_sent {
            append_synthetic(&engine, conversation_id, MAX_STEPS_PROMPT).await?;
            max_notice_sent = true;
            tools_disabled = true;
            final_step = true;
            tool_choice = ToolChoice::None;
        }
        let _ = engine.events.send(CoreEvent::Step {
            conversation_id: conversation_id.to_owned(),
            step,
            max_steps,
        });

        let history = messages::list_for_conversation(&engine.db.pool, conversation_id).await?;
        let (mut chat_messages, summary) = build_messages(&history);
        let mut system = resolved.system.clone();
        if tool_mode == "lazy" {
            if let Some(note) = lazy_tool_note(&registry) {
                let base = system.unwrap_or_default();
                system = Some(format!(
                    "{base}{}{note}",
                    if base.is_empty() { "" } else { "\n\n" }
                ));
            }
        }
        if let Some(summary) = summary {
            let base = system.unwrap_or_default();
            system = Some(format!(
                "{base}{}Summary of earlier conversation:\n{summary}",
                if base.is_empty() { "" } else { "\n\n" }
            ));
        }
        let mut estimate = estimate_tokens(&chat_messages, &tool_defs, system.as_deref());
        let context_window = resolved.context_window as usize;
        let configured_buffer = settings::get_i64(&engine.db.pool, "compaction.buffer", 0)
            .await
            .unwrap_or(0);
        let mut buffer = if configured_buffer > 0 {
            configured_buffer as usize
        } else {
            (context_window * 15 / 100).max(16_000)
        };
        if buffer >= context_window {
            buffer = 0;
        }
        let threshold = context_window.saturating_sub(buffer).max(1);

        if estimate > threshold {
            match compact(&engine, &conversation, &resolved, &cancel).await {
                Ok(()) => {
                    compacted_once = true;
                    let history =
                        messages::list_for_conversation(&engine.db.pool, conversation_id).await?;
                    let (messages, summary) = build_messages(&history);
                    chat_messages = messages;
                    if let Some(summary) = summary {
                        let base = resolved.system.clone().unwrap_or_default();
                        system = Some(format!(
                            "{base}{}Summary of earlier conversation:\n{summary}",
                            if base.is_empty() { "" } else { "\n\n" }
                        ));
                    }
                    estimate = estimate_tokens(&chat_messages, &tool_defs, system.as_deref());
                }
                Err(error) => {
                    tracing::warn!(error = %error.user_message(), "compaction failed");
                }
            }
        }
        if estimate > resolved.context_window as usize {
            return Err(CoreError::ContextOverflow);
        }
        let _ = engine.events.send(CoreEvent::Usage {
            conversation_id: conversation_id.to_owned(),
            prompt_tokens: estimate as i64,
            completion_tokens: 0,
            context_window: resolved.context_window,
        });

        let request = ChatRequest {
            kind: resolved.kind,
            base_url: resolved.base_url.clone(),
            api_key: resolved.api_key.clone(),
            model: resolved.model.clone(),
            system: system.clone(),
            messages: chat_messages,
            tools: tool_defs.clone(),
            tool_choice,
            max_tokens: resolved.max_tokens,
            temperature: None,
        };

        let mut run = run_stream(
            &engine,
            conversation_id,
            request,
            &registry,
            &defaults,
            &active_server_slugs,
            tools_disabled,
            estimate as i64,
            resolved.context_window,
            cancel.clone(),
        )
        .await;
        if cancel.is_cancelled() {
            run.cancelled = true;
        }

        if run.cancelled {
            if let Some(message_id) = &run.message_id {
                let _ =
                    messages::set_status(&engine.db.pool, message_id, "interrupted", None).await;
                let _ = engine.events.send(CoreEvent::MessageEnd {
                    conversation_id: conversation_id.to_owned(),
                    message_id: message_id.clone(),
                    status: "interrupted".into(),
                    error: None,
                });
            }
            return Ok(());
        }

        if let Some(error) = run.error {
            let step_error = StepError::from_core(&error);
            if step_error.context_overflow && !compacted_once {
                compacted_once = true;
                if let Ok(()) = compact(&engine, &conversation, &resolved, &cancel).await {
                    continue;
                }
            }
            if run.had_output {
                if let Some(message_id) = &run.message_id {
                    let _ = messages::set_status(
                        &engine.db.pool,
                        message_id,
                        "interrupted",
                        Some(&step_error.message),
                    )
                    .await;
                    let _ = engine.events.send(CoreEvent::MessageEnd {
                        conversation_id: conversation_id.to_owned(),
                        message_id: message_id.clone(),
                        status: "interrupted".into(),
                        error: Some(step_error.message.clone()),
                    });
                }
                if step_error.retryable {
                    append_synthetic(&engine, conversation_id, CONTINUE_PROMPT).await?;
                    step += 1;
                    retries = 0;
                    continue;
                }
                fail_turn(engine.clone(), conversation_id, run.message_id, &error).await;
                return Ok(());
            }
            if step_error.retryable && retries < 3 {
                let delay = retry_delay(retries);
                retries += 1;
                tokio::time::sleep(Duration::from_millis(delay.delay_ms)).await;
                recover_used = true;
                continue;
            }
            if step_error.auth {
                fail_turn(engine.clone(), conversation_id, run.message_id, &error).await;
                return Ok(());
            }
            if recover_used && step_error.retryable {
                fail_turn(engine.clone(), conversation_id, run.message_id, &error).await;
                return Ok(());
            }
            fail_turn(engine.clone(), conversation_id, run.message_id, &error).await;
            return Ok(());
        }

        retries = 0;
        if let Some(message_id) = &run.message_id {
            let finish = run.finish.clone();
            if is_length(&finish.unwrap_or(FinishReason::Stop)) {
                length_count += 1;
                if length_count > 2 {
                    let error = CoreError::OutputLimit;
                    fail_turn(engine.clone(), conversation_id, run.message_id, &error).await;
                    return Ok(());
                }
                let _ = messages::set_status(&engine.db.pool, message_id, "completed", None).await;
                let _ = engine.events.send(CoreEvent::MessageEnd {
                    conversation_id: conversation_id.to_owned(),
                    message_id: message_id.clone(),
                    status: "completed".into(),
                    error: None,
                });
                append_synthetic(&engine, conversation_id, OUTPUT_LIMIT_PROMPT).await?;
                step += 1;
                continue;
            }
            let _ = messages::set_status(&engine.db.pool, message_id, "completed", None).await;
            let _ = engine.events.send(CoreEvent::MessageEnd {
                conversation_id: conversation_id.to_owned(),
                message_id: message_id.clone(),
                status: "completed".into(),
                error: None,
            });
        }

        let mut repeated = false;
        for signature in &run.tool_signatures {
            let count = tool_signature_counts.entry(*signature).or_insert(0);
            *count += 1;
            if *count >= 3 {
                repeated = true;
            }
        }
        if repeated && !loop_guard_fired && !final_step {
            loop_guard_fired = true;
            append_synthetic(&engine, conversation_id, LOOP_GUARD_PROMPT).await?;
            tools_disabled = true;
            tool_choice = ToolChoice::None;
            final_step = true;
            step += 1;
            continue;
        }

        if run.had_tools {
            if final_step {
                break;
            }
            step += 1;
            length_count = 0;
            continue;
        }
        break;
    }

    conversations::touch(&engine.db.pool, conversation_id).await?;
    let _ = engine.events.send(CoreEvent::ConversationUpdated {
        conversation_id: conversation_id.to_owned(),
    });
    Ok(())
}

fn schema_args_summary(schema: &Value) -> Option<String> {
    let properties = schema.get("properties")?.as_object()?;
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    let mut parts = Vec::new();
    for (name, property) in properties.iter().take(8) {
        let ty = property
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("any");
        let mut part = format!("{name}:{ty}");
        if required.contains(name.as_str()) {
            part.push('*');
        }
        if let Some(values) = property.get("enum").and_then(Value::as_array) {
            let options = values
                .iter()
                .take(3)
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("|");
            if !options.is_empty() {
                part.push('=');
                part.push_str(&options);
            }
        }
        parts.push(part);
    }
    if properties.len() > 8 {
        parts.push("…".into());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

fn tool_catalog(registry: &ToolRegistry) -> (String, bool) {
    let mut catalog = String::new();
    let mut truncated = false;
    for tool in &registry.tools {
        let description = tool.description.replace('\n', " ");
        let description = crate::util::truncate_chars(&description, 180).0;
        let args = schema_args_summary(&tool.input_schema)
            .map(|args| format!(" | args: {args}"))
            .unwrap_or_default();
        let line = format!(
            "- {} :: {} — {}{}\n",
            tool.server_slug, tool.remote_name, description, args
        );
        if catalog.len() + line.len() > 12_000 {
            truncated = true;
            break;
        }
        catalog.push_str(&line);
    }
    (catalog, truncated)
}

fn lazy_tool_note(registry: &ToolRegistry) -> Option<String> {
    if registry.tools.is_empty() {
        return None;
    }
    let (catalog, truncated) = tool_catalog(registry);
    let search_note = if truncated {
        "The catalog is truncated; use mcp_search_tools only for tools not listed."
    } else {
        "All active tools are listed above; do not call mcp_search_tools just to browse."
    };
    Some(format!(
        "MCP tools are available in lazy mode. Use mcp_call_tool directly with server and tool; call mcp_execute when you need several independent tools in one step (max 8). Call mcp_get_tool_schema only when you need the full JSON schema. Request small limits/fields first for tools that can return large data, and page output only when needed. {search_note} A trailing * means required.\n\n{catalog}"
    ))
}

async fn load_tools(
    engine: &ChatEngine,
    conversation: &crate::models::Conversation,
) -> (
    ToolRegistry,
    HashMap<String, Effect>,
    Vec<ToolDef>,
    Vec<String>,
    String,
) {
    let selected_ids = conversations::selected_server_ids(&engine.db.pool, &conversation.id)
        .await
        .unwrap_or_default();
    let servers = engine
        .mcp
        .tools_for_ids(&selected_ids)
        .await
        .unwrap_or_default();
    let mut input = Vec::new();
    let mut defaults = HashMap::new();
    let mut active_slugs = Vec::new();
    for (server, tools) in &servers {
        defaults.insert(
            server.slug.clone(),
            match server.policy.as_str() {
                "auto" => Effect::Allow,
                "deny" => Effect::Deny,
                _ => Effect::Ask,
            },
        );
        if !active_slugs.contains(&server.slug) {
            active_slugs.push(server.slug.clone());
        }
        input.push((
            server.id.clone(),
            server.slug.clone(),
            server.name.clone(),
            tools.clone(),
        ));
    }
    let registry = ToolRegistry::from_servers(&input).unwrap_or_default();
    let tool_mode = conversation.tool_mode.clone();
    let mut defs: Vec<ToolDef> = if tool_mode == "lazy" && !registry.tools.is_empty() {
        let (_, truncated) = tool_catalog(&registry);
        let mut meta = meta_tool_defs();
        if !truncated {
            meta.retain(|definition| definition.name != MCP_SEARCH_TOOLS);
        }
        meta
    } else {
        registry
            .tools
            .iter()
            .map(|tool| ToolDef {
                name: tool.exposed_name.clone(),
                description: if tool.server_name.is_empty() {
                    tool.description.clone()
                } else {
                    format!("{}\n\nServer: {}", tool.description, tool.server_name)
                },
                input_schema: tool.input_schema.clone(),
            })
            .collect()
    };
    defs.extend(builtin_tool_defs());
    (registry, defaults, defs, active_slugs, tool_mode)
}

#[allow(clippy::too_many_arguments)]
async fn run_stream(
    engine: &ChatEngine,
    conversation_id: &str,
    request: ChatRequest,
    registry: &ToolRegistry,
    defaults: &HashMap<String, Effect>,
    active_server_slugs: &[String],
    tools_disabled: bool,
    prompt_tokens: i64,
    context_window: i64,
    cancel: CancellationToken,
) -> StreamRun {
    let mut result = StreamRun {
        message_id: None,
        finish: None,
        error: None,
        cancelled: false,
        had_output: false,
        had_tools: false,
        tool_signatures: Vec::new(),
    };
    let provider = llm::provider_for(request.kind);
    let mut stream = match provider.stream(request, cancel.clone()).await {
        Ok(stream) => stream,
        Err(error) => {
            result.error = Some(error);
            return result;
        }
    };

    let mut text = PartWriter::new("text");
    let mut reasoning = PartWriter::new("reasoning");
    let mut message_id: Option<String> = None;
    let mut calls: BTreeMap<usize, CallState> = BTreeMap::new();
    let mut handles: Vec<(usize, JoinHandle<crate::agent::tools::ToolExecResult>)> = Vec::new();
    let mut tool_order: Vec<usize> = Vec::new();
    let mut usage_emitted = false;

    loop {
        let item = tokio::select! {
            _ = cancel.cancelled() => {
                result.cancelled = true;
                break;
            }
            item = stream.next() => item,
        };
        let Some(item) = item else {
            result
                .finish
                .get_or_insert(FinishReason::Unknown("end_of_stream".into()));
            break;
        };
        match item {
            Err(error) => {
                result.error = Some(error);
                break;
            }
            Ok(Delta::Text(delta)) => {
                if let Some(id) = ensure_message(engine, conversation_id, &mut message_id).await {
                    if let Err(error) = text.push(engine, conversation_id, &id, &delta).await {
                        result.error = Some(error);
                        break;
                    }
                    result.had_output = true;
                }
            }
            Ok(Delta::Reasoning(delta)) => {
                if let Some(id) = ensure_message(engine, conversation_id, &mut message_id).await {
                    if let Err(error) = reasoning.push(engine, conversation_id, &id, &delta).await {
                        result.error = Some(error);
                        break;
                    }
                    result.had_output = true;
                }
            }
            Ok(Delta::ToolCallStart { index, id, name }) => {
                let Some(message) = ensure_message(engine, conversation_id, &mut message_id).await
                else {
                    continue;
                };
                result.had_output = true;
                result.had_tools = true;
                let call = calls.entry(index).or_insert_with(|| CallState {
                    index,
                    id: id.clone(),
                    name: name.clone(),
                    fragment: String::new(),
                    args: None,
                    part_id: None,
                    dispatched: false,
                    argued: false,
                });
                call.id = id;
                call.name = name.clone();
                call.argued = true;
                let server_label = if name == "read_tool_output" {
                    "mcp-chat".to_owned()
                } else {
                    registry
                        .get(&name)
                        .map(|tool| tool.server_name.clone())
                        .unwrap_or_default()
                };
                let part = create_tool_part(
                    engine,
                    conversation_id,
                    &message,
                    &call.id,
                    &call.name,
                    &server_label,
                    json!({}),
                )
                .await;
                if let Ok(part) = part {
                    call.part_id = Some(part.id);
                }
                if !tool_order.contains(&index) {
                    tool_order.push(index);
                }
            }
            Ok(Delta::ToolCallArgs {
                index,
                id,
                fragment,
            }) => {
                let Some(message) = ensure_message(engine, conversation_id, &mut message_id).await
                else {
                    continue;
                };
                result.had_output = true;
                result.had_tools = true;
                let call = calls.entry(index).or_insert_with(|| CallState {
                    index,
                    id: id.clone(),
                    name: "tool".into(),
                    fragment: String::new(),
                    args: None,
                    part_id: None,
                    dispatched: false,
                    argued: false,
                });
                call.fragment.push_str(&fragment);
                if call.part_id.is_none() {
                    if let Ok(part) = create_tool_part(
                        engine,
                        conversation_id,
                        &message,
                        &call.id,
                        &call.name,
                        "",
                        json!({}),
                    )
                    .await
                    {
                        call.part_id = Some(part.id);
                    }
                }
                if !tool_order.contains(&index) {
                    tool_order.push(index);
                }
                if call.args.is_none() {
                    if let Some(args) = parse_args(&call.fragment) {
                        call.args = Some(args.clone());
                        if let Err(error) = dispatch_call(
                            engine,
                            conversation_id,
                            message_id.as_deref(),
                            registry,
                            defaults,
                            active_server_slugs,
                            tools_disabled,
                            cancel.clone(),
                            call,
                            &mut handles,
                        )
                        .await
                        {
                            result.error = Some(error);
                            break;
                        }
                    }
                }
            }
            Ok(Delta::ToolCallEnd { index, id }) => {
                if let Some(call) = calls.get_mut(&index) {
                    if call.id.is_empty() {
                        call.id = id;
                    }
                    if call.args.is_none() {
                        if let Some(args) = parse_args(&call.fragment) {
                            call.args = Some(args);
                        }
                    }
                }
            }
            Ok(Delta::Usage(usage)) => {
                let _ = engine.events.send(CoreEvent::Usage {
                    conversation_id: conversation_id.to_owned(),
                    prompt_tokens: if usage.prompt_tokens > 0 {
                        usage.prompt_tokens
                    } else {
                        prompt_tokens
                    },
                    completion_tokens: usage.completion_tokens,
                    context_window,
                });
                usage_emitted = true;
            }
            Ok(Delta::Done(reason)) => {
                result.finish = Some(reason);
                break;
            }
        }
    }

    if let Some(message) = &message_id {
        let _ = text.flush(engine).await;
        let _ = reasoning.flush(engine).await;
        let _ = message;
    }

    // Settle every call exactly once.
    let mut outcomes: BTreeMap<usize, crate::agent::tools::ToolExecResult> = BTreeMap::new();
    if result.cancelled || result.error.is_some() {
        for index in tool_order.clone() {
            let call = calls.get_mut(&index).expect("call");
            if !call.dispatched {
                let message = if result.cancelled {
                    "Tool execution interrupted"
                } else if !call.fragment.trim().is_empty() && call.args.is_none() {
                    "Invalid tool input: arguments are not valid JSON."
                } else {
                    "Provider did not return a tool result"
                };
                let outcome = settle_call_error(
                    engine,
                    conversation_id,
                    registry,
                    defaults,
                    active_server_slugs,
                    call,
                    message,
                )
                .await;
                outcomes.insert(index, outcome);
            }
        }
    } else {
        for index in tool_order.clone() {
            let call = calls.get_mut(&index).expect("call");
            if !call.dispatched {
                if call.args.is_none() {
                    if call.fragment.trim().is_empty() {
                        call.args = Some(json!({}));
                    } else if let Some(args) = parse_args(&call.fragment) {
                        call.args = Some(args);
                    } else {
                        let outcome = settle_call_error(
                            engine,
                            conversation_id,
                            registry,
                            defaults,
                            active_server_slugs,
                            call,
                            "Invalid tool input: arguments are not valid JSON.",
                        )
                        .await;
                        outcomes.insert(index, outcome);
                        continue;
                    }
                }
                let _ = dispatch_call(
                    engine,
                    conversation_id,
                    message_id.as_deref(),
                    registry,
                    defaults,
                    active_server_slugs,
                    tools_disabled,
                    cancel.clone(),
                    call,
                    &mut handles,
                )
                .await;
            }
        }
    }

    for (index, handle) in handles {
        match handle.await {
            Ok(outcome) => {
                outcomes.insert(outcome.index, outcome);
            }
            Err(_) => {
                if let Some(call) = calls.get_mut(&index) {
                    let outcome = settle_call_error(
                        engine,
                        conversation_id,
                        registry,
                        defaults,
                        active_server_slugs,
                        call,
                        "The tool task crashed before returning a result.",
                    )
                    .await;
                    outcomes.insert(index, outcome);
                }
            }
        }
    }

    for index in &tool_order {
        if !outcomes.contains_key(index) {
            let call = calls.get_mut(index).expect("call");
            let outcome = settle_call_error(
                engine,
                conversation_id,
                registry,
                defaults,
                active_server_slugs,
                call,
                "Provider did not return a tool result",
            )
            .await;
            outcomes.insert(*index, outcome);
        }
    }

    for index in &tool_order {
        if let Some(outcome) = outcomes.get(index) {
            if let Some(message) = &message_id {
                let _ = persist_result(engine.executor.as_ref(), conversation_id, message, outcome)
                    .await;
            }
            let call = calls.get(index);
            let name = call.map(|call| call.name.as_str()).unwrap_or("");
            let args = call
                .and_then(|call| call.args.clone())
                .unwrap_or_else(|| json!({}));
            result
                .tool_signatures
                .push(tool_signature(name, &args, outcome.ok, &outcome.content));
        }
    }

    if !usage_emitted {
        let completion = ((text.text.chars().count() + reasoning.text.chars().count()) / 3) as i64;
        if completion > 0 {
            let _ = engine.events.send(CoreEvent::Usage {
                conversation_id: conversation_id.to_owned(),
                prompt_tokens,
                completion_tokens: completion,
                context_window,
            });
        }
    }

    result.message_id = message_id;
    result
}

async fn ensure_message(
    engine: &ChatEngine,
    conversation_id: &str,
    message_id: &mut Option<String>,
) -> Option<String> {
    if let Some(id) = message_id {
        return Some(id.clone());
    }
    match messages::create(&engine.db.pool, conversation_id, "assistant", "streaming").await {
        Ok(message) => {
            *message_id = Some(message.id.clone());
            let _ = engine.events.send(CoreEvent::MessageStart {
                conversation_id: conversation_id.to_owned(),
                message_id: message.id.clone(),
            });
            let _ = engine.events.send(CoreEvent::Status {
                conversation_id: conversation_id.to_owned(),
                status: TurnStatus::Streaming,
                message: None,
            });
            Some(message.id)
        }
        Err(_) => None,
    }
}

async fn create_tool_part(
    engine: &ChatEngine,
    conversation_id: &str,
    message_id: &str,
    call_id: &str,
    name: &str,
    server: &str,
    args: Value,
) -> Result<crate::models::Part> {
    let payload = json!({
        "callId": call_id,
        "name": name,
        "server": server,
        "args": args,
        "state": "streaming",
        "startedAt": now_ms(),
        "endedAt": null,
        "executed": false,
        "approval": null,
    });
    let part =
        messages::append_raw_part(&engine.db.pool, message_id, "tool_call", &payload).await?;
    let _ = engine.events.send(CoreEvent::Part {
        conversation_id: conversation_id.to_owned(),
        message_id: message_id.to_owned(),
        part_id: part.id.clone(),
        kind: "tool_call".into(),
        payload: payload.clone(),
    });
    Ok(part)
}

fn parse_args(fragment: &str) -> Option<Value> {
    let trimmed = fragment.trim();
    if trimmed.is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) if value.is_object() => Some(value),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_call(
    engine: &ChatEngine,
    conversation_id: &str,
    message_id: Option<&str>,
    registry: &ToolRegistry,
    defaults: &HashMap<String, Effect>,
    active_server_slugs: &[String],
    tools_disabled: bool,
    cancel: CancellationToken,
    call: &mut CallState,
    handles: &mut Vec<(usize, JoinHandle<crate::agent::tools::ToolExecResult>)>,
) -> Result<()> {
    if call.dispatched {
        return Ok(());
    }
    let Some(args) = call.args.clone() else {
        return Ok(());
    };
    call.dispatched = true;
    let (server_id, slug, server_name, default, tool) = if is_builtin_tool(&call.name) {
        (
            String::new(),
            "builtin".to_owned(),
            "mcp-chat".to_owned(),
            builtin_default_effect(&call.name),
            builtin_tool_info(&call.name),
        )
    } else if is_meta_tool(&call.name) {
        (
            String::new(),
            "builtin".to_owned(),
            "mcp-chat".to_owned(),
            Effect::Allow,
            meta_tool_info(&call.name),
        )
    } else if let Some(tool) = registry.get(&call.name) {
        (
            tool.server_id.clone(),
            tool.server_slug.clone(),
            tool.server_name.clone(),
            defaults
                .get(&tool.server_slug)
                .copied()
                .unwrap_or(Effect::Ask),
            Some(tool.to_info()),
        )
    } else {
        (
            String::new(),
            "unknown".to_owned(),
            "unknown".to_owned(),
            Effect::Ask,
            None,
        )
    };
    let part_id = call.part_id.clone().unwrap_or_default();
    if part_id.is_empty() {
        return Ok(());
    }
    let payload = json!({
        "callId": call.id,
        "name": call.name,
        "server": server_name,
        "args": args,
        "state": "queued",
        "startedAt": now_ms(),
        "endedAt": null,
        "executed": false,
        "approval": null,
    });
    let _ = messages::update_part_payload(&engine.db.pool, &part_id, &payload).await;
    if let Some(message_id) = message_id {
        let _ = engine.events.send(CoreEvent::Part {
            conversation_id: conversation_id.to_owned(),
            message_id: message_id.to_owned(),
            part_id: part_id.clone(),
            kind: "tool_call".into(),
            payload: payload.clone(),
        });
    }
    let remote_name = tool
        .as_ref()
        .map(|tool| tool.name.clone())
        .unwrap_or_else(|| call.name.clone());
    let request = ToolExecRequest {
        conversation_id: conversation_id.to_owned(),
        message_id: String::new(),
        part_id,
        call_id: call.id.clone(),
        index: call.index,
        exposed_name: call.name.clone(),
        remote_name,
        server_id,
        server_slug: slug,
        server_name,
        args,
        tool,
        default_effect: default,
        disabled: tools_disabled,
        cancel,
        active_server_slugs: active_server_slugs.to_vec(),
    };
    let executor = engine.executor.clone();
    let handle = tokio::spawn(async move { executor.execute(request).await });
    handles.push((call.index, handle));
    Ok(())
}

async fn settle_call_error(
    engine: &ChatEngine,
    conversation_id: &str,
    registry: &ToolRegistry,
    defaults: &HashMap<String, Effect>,
    active_server_slugs: &[String],
    call: &mut CallState,
    message: &str,
) -> crate::agent::tools::ToolExecResult {
    let (server_id, slug, server_name, default, tool) = if is_builtin_tool(&call.name) {
        (
            String::new(),
            "builtin".to_owned(),
            "mcp-chat".to_owned(),
            builtin_default_effect(&call.name),
            builtin_tool_info(&call.name),
        )
    } else if is_meta_tool(&call.name) {
        (
            String::new(),
            "builtin".to_owned(),
            "mcp-chat".to_owned(),
            Effect::Allow,
            meta_tool_info(&call.name),
        )
    } else if let Some(tool) = registry.get(&call.name) {
        (
            tool.server_id.clone(),
            tool.server_slug.clone(),
            tool.server_name.clone(),
            defaults
                .get(&tool.server_slug)
                .copied()
                .unwrap_or(Effect::Ask),
            Some(tool.to_info()),
        )
    } else {
        (
            String::new(),
            "unknown".to_owned(),
            "unknown".to_owned(),
            Effect::Ask,
            None,
        )
    };
    let remote_name = tool
        .as_ref()
        .map(|tool| tool.name.clone())
        .unwrap_or_else(|| call.name.clone());
    let request = ToolExecRequest {
        conversation_id: conversation_id.to_owned(),
        message_id: String::new(),
        part_id: call.part_id.clone().unwrap_or_default(),
        call_id: call.id.clone(),
        index: call.index,
        exposed_name: call.name.clone(),
        remote_name,
        server_id,
        server_slug: slug,
        server_name,
        args: call.args.clone().unwrap_or_else(|| json!({})),
        tool,
        default_effect: default,
        disabled: false,
        cancel: CancellationToken::new(),
        active_server_slugs: active_server_slugs.to_vec(),
    };
    call.dispatched = true;
    let state = if message.contains("interrupted") {
        "aborted"
    } else {
        "error"
    };
    engine
        .executor
        .settle_error(request, state, message.to_owned())
        .await
}

async fn append_synthetic(engine: &ChatEngine, conversation_id: &str, text: &str) -> Result<()> {
    let message = messages::create(&engine.db.pool, conversation_id, "user", "synthetic").await?;
    messages::append_part(
        &engine.db.pool,
        &message.id,
        "text",
        &PartPayload::Text {
            text: text.to_owned(),
        },
    )
    .await?;
    messages::append_part(
        &engine.db.pool,
        &message.id,
        "time",
        &PartPayload::Text {
            text: crate::util::message_time_note(message.created_at),
        },
    )
    .await?;
    conversations::touch(&engine.db.pool, conversation_id).await?;
    let _ = engine.events.send(CoreEvent::ConversationUpdated {
        conversation_id: conversation_id.to_owned(),
    });
    Ok(())
}

async fn fail_turn(
    engine: Arc<ChatEngine>,
    conversation_id: &str,
    message_id: Option<String>,
    error: &CoreError,
) {
    let message = error.user_message();
    let message_id = match message_id {
        Some(id) => id,
        None => {
            match messages::create(&engine.db.pool, conversation_id, "assistant", "error").await {
                Ok(message) => {
                    let _ = engine.events.send(CoreEvent::MessageStart {
                        conversation_id: conversation_id.to_owned(),
                        message_id: message.id.clone(),
                    });
                    message.id
                }
                Err(_) => return,
            }
        }
    };
    let _ = messages::set_status(&engine.db.pool, &message_id, "error", Some(&message)).await;
    if let Ok(part) = messages::append_part(
        &engine.db.pool,
        &message_id,
        "error",
        &PartPayload::Error {
            message: message.clone(),
        },
    )
    .await
    {
        let _ = engine.events.send(CoreEvent::Part {
            conversation_id: conversation_id.to_owned(),
            message_id: message_id.clone(),
            part_id: part.id,
            kind: "error".into(),
            payload: json!({"message": message}),
        });
    }
    let _ = engine.events.send(CoreEvent::MessageEnd {
        conversation_id: conversation_id.to_owned(),
        message_id,
        status: "error".into(),
        error: Some(message.clone()),
    });
    let _ = engine.events.send(CoreEvent::Status {
        conversation_id: conversation_id.to_owned(),
        status: TurnStatus::Error,
        message: Some(message.clone()),
    });
}

async fn compact(
    engine: &ChatEngine,
    conversation: &crate::models::Conversation,
    resolved: &Resolved,
    cancel: &CancellationToken,
) -> Result<()> {
    let history = messages::list_for_conversation(&engine.db.pool, &conversation.id).await?;
    if history.len() < 4 {
        return Err(CoreError::Message("Not enough history to compact.".into()));
    }
    let keep_tokens = settings::get_i64(&engine.db.pool, "compaction.keep_tokens", 15_000)
        .await
        .unwrap_or(15_000)
        .max(1_000) as usize;
    let keep_tokens = keep_tokens
        .min((resolved.context_window as usize) * 2 / 3)
        .max(1_000);
    let mut keep_start = history.len();
    let mut kept = 0usize;
    for index in (0..history.len()).rev() {
        if kept >= keep_tokens && keep_start < history.len() {
            break;
        }
        kept += message_tokens(&history[index]);
        keep_start = index;
    }
    // Even when the whole transcript is smaller than keep_tokens, context
    // overflow still needs a boundary. Keep at least the last two messages.
    if keep_start == 0 {
        keep_start = history.len().saturating_sub(2);
    }
    while keep_start > 0 && history[keep_start].message.role == "tool" {
        keep_start -= 1;
    }
    if keep_start == 0 {
        return Err(CoreError::Message("Not enough history to compact.".into()));
    }
    let boundary = &history[keep_start - 1];
    let transcript = transcript_for_summary(&history[..keep_start]);
    if transcript.trim().is_empty() {
        return Err(CoreError::Message("Nothing to summarize.".into()));
    }
    let request = ChatRequest {
        kind: resolved.kind,
        base_url: resolved.base_url.clone(),
        api_key: resolved.api_key.clone(),
        model: resolved.model.clone(),
        system: Some(
            "You are a precise conversation compactor. Preserve decisions, facts, user preferences, file paths, tool results and open tasks. Be compact but complete. Do not add commentary.".into(),
        ),
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: vec![ContentBlock::Text(format!(
                "Summarize this conversation transcript:\n\n{transcript}"
            ))],
        }],
        tools: Vec::new(),
        tool_choice: ToolChoice::None,
        max_tokens: resolved.max_tokens.min(2048),
        temperature: None,
    };
    let summary = llm::complete(resolved.kind, request, cancel.clone()).await?;
    if summary.trim().is_empty() {
        return Err(CoreError::Message(
            "The summarizer returned an empty result.".into(),
        ));
    }
    let message =
        messages::create(&engine.db.pool, &conversation.id, "system", "completed").await?;
    messages::append_part(
        &engine.db.pool,
        &message.id,
        "summary",
        &PartPayload::Summary {
            text: summary,
            covers_until: boundary.message.id.clone(),
        },
    )
    .await?;
    conversations::touch(&engine.db.pool, &conversation.id).await?;
    let _ = engine.events.send(CoreEvent::ConversationUpdated {
        conversation_id: conversation.id.clone(),
    });
    Ok(())
}

fn message_tokens(message: &crate::models::MessageWithParts) -> usize {
    let mut chars = 0usize;
    for part in &message.parts {
        let payload = part.payload();
        match part.kind.as_str() {
            "text" => {
                chars += payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .chars()
                    .count()
            }
            "tool_call" => {
                chars += serde_json::to_string(&payload.get("args").cloned().unwrap_or(Value::Null))
                    .map(|value| value.len())
                    .unwrap_or(0)
            }
            "tool_result" => {
                chars += payload
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .chars()
                    .count()
            }
            _ => {}
        }
    }
    (chars / 3).max(1)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextStatus {
    pub tokens: i64,
    pub window: i64,
    pub threshold: i64,
    pub keep_tokens: i64,
    pub tool_count: usize,
    pub tool_mode: String,
}

pub async fn context_status(
    engine: Arc<ChatEngine>,
    conversation_id: &str,
) -> Result<ContextStatus> {
    let conversation = conversations::get(&engine.db.pool, conversation_id).await?;
    let (registry, _defaults, tool_defs, _slugs, tool_mode) =
        load_tools(&engine, &conversation).await;
    let (_, _, _, context_window, _) = engine.resolve(&conversation).await?;
    let history = messages::list_for_conversation(&engine.db.pool, conversation_id).await?;
    let (chat_messages, summary) = build_messages(&history);
    let mut system = resolve_system_prompt(&engine, &conversation).await?;
    if let Some(summary) = summary {
        let base = system.unwrap_or_default();
        system = Some(format!(
            "{base}{}Summary of earlier conversation:\n{summary}",
            if base.is_empty() { "" } else { "\n\n" }
        ));
    }
    if tool_mode == "lazy" {
        if let Some(note) = lazy_tool_note(&registry) {
            let base = system.unwrap_or_default();
            system = Some(format!(
                "{base}{}{note}",
                if base.is_empty() { "" } else { "\n\n" }
            ));
        }
    }
    let tokens = estimate_tokens(&chat_messages, &tool_defs, system.as_deref()) as i64;
    let window = context_window;
    let keep_tokens = settings::get_i64(&engine.db.pool, "compaction.keep_tokens", 15_000)
        .await
        .unwrap_or(15_000)
        .max(1_000);
    let configured_buffer = settings::get_i64(&engine.db.pool, "compaction.buffer", 0)
        .await
        .unwrap_or(0);
    let mut buffer = if configured_buffer > 0 {
        configured_buffer
    } else {
        (window * 15 / 100).max(16_000)
    };
    if buffer >= window {
        buffer = 0;
    }
    Ok(ContextStatus {
        tokens,
        window,
        threshold: (window - buffer).max(1),
        keep_tokens,
        tool_count: registry.tools.len(),
        tool_mode,
    })
}

pub async fn compact_now(engine: Arc<ChatEngine>, conversation_id: &str) -> Result<()> {
    if engine.in_flight(conversation_id) {
        return Err(CoreError::Message(
            "Stop the current turn before compacting.".into(),
        ));
    }
    let conversation = conversations::get(&engine.db.pool, conversation_id).await?;
    let (provider, model, api_key, context_window, max_tokens) =
        engine.resolve(&conversation).await?;
    let kind = ProviderKind::parse(&provider.kind)
        .ok_or_else(|| CoreError::Message("Unknown provider kind.".into()))?;
    let system = resolve_system_prompt(&engine, &conversation).await?;
    let resolved = Resolved {
        kind,
        base_url: provider.base_url.clone(),
        api_key,
        model,
        context_window,
        max_tokens,
        system,
    };
    compact(&engine, &conversation, &resolved, &CancellationToken::new()).await
}
