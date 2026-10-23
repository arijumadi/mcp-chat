use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::llm::provider::{
    ChatRequest, ChatRole, ContentBlock, Delta, DeltaStream, FinishReason, Provider, http_client,
    openai_url, parse_finish_reason, response_error,
};
use crate::llm::sse::{SseEvent, SseParser};
use crate::models::Usage;
use crate::{CoreError, Result};

pub struct OpenAiProvider;

#[async_trait]
impl Provider for OpenAiProvider {
    async fn stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<DeltaStream> {
        let client = http_client()?;
        let url = openai_url(&req.base_url);
        let body = build_body(&req)?;
        let mut builder = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");
        if let Some(key) = req.api_key.as_deref() {
            builder = builder.bearer_auth(key);
        }
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
            response = builder.json(&body).send() => response?,
        };
        if !response.status().is_success() {
            return Err(response_error(response).await);
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();

        if content_type.contains("application/json") {
            let body = response.text().await?;
            return json_fallback(&body);
        }

        let stream = response.bytes_stream();
        let state = OpenAiState {
            stream: Box::pin(stream),
            parser: SseParser::new(),
            pending: VecDeque::new(),
            tools: HashMap::new(),
            started: HashSet::new(),
            finished: false,
            usage_emitted: false,
        };
        let stream = futures::stream::unfold(state, |mut state| async move {
            let item = next_item(&mut state).await?;
            Some((item, state))
        });
        Ok(Box::pin(stream))
    }
}

struct OpenAiState {
    stream: Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>,
    parser: SseParser,
    pending: VecDeque<Result<Delta>>,
    tools: HashMap<usize, (String, String)>,
    started: HashSet<usize>,
    finished: bool,
    usage_emitted: bool,
}

async fn next_item(state: &mut OpenAiState) -> Option<Result<Delta>> {
    loop {
        if let Some(item) = state.pending.pop_front() {
            return Some(item);
        }
        if state.finished {
            return None;
        }
        match state.stream.next().await {
            Some(Ok(bytes)) => {
                for event in state.parser.push(&bytes) {
                    handle_event(state, event);
                }
            }
            Some(Err(err)) => {
                state.finished = true;
                return Some(Err(CoreError::Http(err)));
            }
            None => {
                if let Some(event) = state.parser.finish() {
                    handle_event(state, event);
                }
                state.finished = true;
                if state.pending.is_empty() {
                    return Some(Ok(Delta::Done(FinishReason::Unknown(
                        "end_of_stream".into(),
                    ))));
                }
            }
        }
    }
}

fn handle_event(state: &mut OpenAiState, event: SseEvent) {
    let data = event.data.trim();
    if data.is_empty() {
        return;
    }
    if data == "[DONE]" {
        if state.pending.is_empty() {
            state
                .pending
                .push_back(Ok(Delta::Done(FinishReason::Unknown(
                    "done_without_finish_reason".into(),
                ))));
        }
        return;
    }
    let value: Value = match serde_json::from_str(data) {
        Ok(value) => value,
        Err(_) => {
            state.pending.push_back(Err(CoreError::message(
                "The provider sent a malformed streaming chunk.",
            )));
            return;
        }
    };
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The provider returned an error.");
        let err = if crate::error::is_context_overflow_message(message) {
            CoreError::ContextOverflow
        } else {
            CoreError::message(message.to_owned())
        };
        state.pending.push_back(Err(err));
        state.finished = true;
        return;
    }

    if let Some(usage) = value.get("usage").and_then(Usage::from_json) {
        if !state.usage_emitted && (usage.prompt_tokens > 0 || usage.completion_tokens > 0) {
            state.usage_emitted = true;
            state.pending.push_back(Ok(Delta::Usage(usage)));
        }
    }

    let Some(choice) = value.get("choices").and_then(|c| c.get(0)) else {
        return;
    };
    if let Some(delta) = choice.get("delta") {
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                state.pending.push_back(Ok(Delta::Text(text.to_owned())));
            }
        }
        if let Some(reasoning) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
        {
            if !reasoning.is_empty() {
                state
                    .pending
                    .push_back(Ok(Delta::Reasoning(reasoning.to_owned())));
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                handle_tool_delta(state, call);
            }
        }
    }
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        if !state.finished {
            for index in state.started.iter() {
                if let Some((id, _)) = state.tools.get(index) {
                    state.pending.push_back(Ok(Delta::ToolCallEnd {
                        index: *index,
                        id: id.clone(),
                    }));
                }
            }
            state
                .pending
                .push_back(Ok(Delta::Done(parse_finish_reason(reason))));
            state.finished = true;
        }
    }
}

fn handle_tool_delta(state: &mut OpenAiState, call: &Value) {
    let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    let id = call
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| state.tools.get(&index).map(|(id, _)| id.clone()))
        .unwrap_or_else(|| format!("call_{index}"));
    let name = call
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| state.tools.get(&index).map(|(_, name)| name.clone()));

    let Some(name) = name else {
        return;
    };
    if !state.started.contains(&index) {
        state.started.insert(index);
        state.tools.insert(index, (id.clone(), name.clone()));
        state.pending.push_back(Ok(Delta::ToolCallStart {
            index,
            id: id.clone(),
            name,
        }));
    }
    if let Some(fragment) = call
        .get("function")
        .and_then(|f| f.get("arguments"))
        .and_then(Value::as_str)
    {
        if !fragment.is_empty() {
            state.pending.push_back(Ok(Delta::ToolCallArgs {
                index,
                id,
                fragment: fragment.to_owned(),
            }));
        }
    }
}

fn json_fallback(body: &str) -> Result<DeltaStream> {
    let value: Value = serde_json::from_str(body)?;
    let mut deltas = Vec::new();
    if let Some(usage) = value.get("usage").and_then(Usage::from_json) {
        deltas.push(Ok(Delta::Usage(usage)));
    }
    let choice = value
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| CoreError::message("The provider returned no choices."))?;
    if let Some(message) = choice.get("message") {
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            deltas.push(Ok(Delta::Text(text.to_owned())));
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for (index, call) in calls.iter().enumerate() {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("call_0")
                    .to_owned();
                let name = call
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_owned();
                deltas.push(Ok(Delta::ToolCallStart {
                    index,
                    id: id.clone(),
                    name,
                }));
                let args = call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                deltas.push(Ok(Delta::ToolCallArgs {
                    index,
                    id: id.clone(),
                    fragment: args.to_owned(),
                }));
                deltas.push(Ok(Delta::ToolCallEnd { index, id }));
            }
        }
    }
    let reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop");
    deltas.push(Ok(Delta::Done(parse_finish_reason(reason))));
    Ok(Box::pin(futures::stream::iter(deltas)))
}

fn build_body(req: &ChatRequest) -> Result<Value> {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = req.system.as_deref().filter(|s| !s.trim().is_empty()) {
        messages.push(json!({"role": "system", "content": system}));
    }
    for message in &req.messages {
        match message.role {
            ChatRole::System => {
                messages.push(json!({"role": "system", "content": message.text_content()}));
            }
            ChatRole::User => {
                messages.push(json!({"role": "user", "content": message.text_content()}));
            }
            ChatRole::Assistant => {
                let text = message.text_content();
                let calls = message.tool_calls();
                let mut object = json!({
                    "role": "assistant",
                    "content": if text.is_empty() { Value::Null } else { Value::String(text) },
                });
                if !calls.is_empty() {
                    object["tool_calls"] = Value::Array(
                        calls
                            .into_iter()
                            .map(|(id, name, arguments)| {
                                json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".into()),
                                    }
                                })
                            })
                            .collect(),
                    );
                }
                messages.push(object);
            }
            ChatRole::Tool => {
                for block in &message.content {
                    if let ContentBlock::ToolResult {
                        call_id,
                        content,
                        is_error,
                        ..
                    } = block
                    {
                        let text = if *is_error {
                            format!("Error: {content}")
                        } else {
                            content.clone()
                        };
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": text,
                        }));
                    }
                }
            }
        }
    }

    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
        "max_tokens": req.max_tokens.max(1),
    });
    if let Some(temperature) = req.temperature {
        body["temperature"] = json!(temperature);
    }
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.input_schema,
                        }
                    })
                })
                .collect(),
        );
        body["tool_choice"] = json!(match req.tool_choice {
            crate::llm::provider::ToolChoice::Auto => "auto",
            crate::llm::provider::ToolChoice::None => "none",
        });
    }
    Ok(body)
}
