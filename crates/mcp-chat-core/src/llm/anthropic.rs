use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::llm::provider::{
    ChatRequest, ChatRole, ContentBlock, Delta, DeltaStream, FinishReason, Provider, ToolChoice,
    anthropic_url, http_client, parse_finish_reason, response_error,
};
use crate::llm::sse::{SseEvent, SseParser};
use crate::models::Usage;
use crate::{CoreError, Result};

pub struct AnthropicProvider;

#[async_trait]
impl Provider for AnthropicProvider {
    async fn stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<DeltaStream> {
        let client = http_client()?;
        let url = anthropic_url(&req.base_url);
        let body = build_body(&req)?;
        let mut builder = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("anthropic-version", "2023-06-01");
        if let Some(key) = req.api_key.as_deref() {
            builder = builder.header("x-api-key", key);
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
        if !content_type.contains("text/event-stream") {
            let body = response.text().await?;
            return json_fallback(&body);
        }

        let state = AnthropicState {
            stream: Box::pin(response.bytes_stream()),
            parser: SseParser::new(),
            pending: VecDeque::new(),
            tools: HashMap::new(),
            emitted_end: HashSet::new(),
            input_tokens: 0,
            output_tokens: 0,
            finish: None,
            done: false,
        };
        let stream = futures::stream::unfold(state, |mut state| async move {
            let item = next_item(&mut state).await?;
            Some((item, state))
        });
        Ok(Box::pin(stream))
    }
}

struct AnthropicState {
    stream: Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>,
    parser: SseParser,
    pending: VecDeque<Result<Delta>>,
    tools: HashMap<usize, String>,
    emitted_end: HashSet<usize>,
    input_tokens: i64,
    output_tokens: i64,
    finish: Option<FinishReason>,
    done: bool,
}

async fn next_item(state: &mut AnthropicState) -> Option<Result<Delta>> {
    loop {
        if let Some(item) = state.pending.pop_front() {
            return Some(item);
        }
        if state.done {
            return None;
        }
        match state.stream.next().await {
            Some(Ok(bytes)) => {
                for event in state.parser.push(&bytes) {
                    handle_event(state, event);
                }
            }
            Some(Err(err)) => {
                state.done = true;
                return Some(Err(CoreError::Http(err)));
            }
            None => {
                if let Some(event) = state.parser.finish() {
                    handle_event(state, event);
                }
                state.done = true;
                if state.pending.is_empty() {
                    return Some(Ok(Delta::Done(
                        state
                            .finish
                            .clone()
                            .unwrap_or(FinishReason::Unknown("end_of_stream".into())),
                    )));
                }
            }
        }
    }
}

fn handle_event(state: &mut AnthropicState, event: SseEvent) {
    let value: Value = match serde_json::from_str(&event.data) {
        Ok(value) => value,
        Err(_) => {
            state.pending.push_back(Err(CoreError::message(
                "The provider sent a malformed streaming event.",
            )));
            return;
        }
    };
    let event_type = event
        .event
        .as_deref()
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("");
    match event_type {
        "message_start" => {
            if let Some(usage) = value
                .get("message")
                .and_then(|m| m.get("usage"))
                .and_then(Usage::from_json)
            {
                state.input_tokens = usage.prompt_tokens;
            }
        }
        "content_block_start" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let block = value.get("content_block").cloned().unwrap_or(Value::Null);
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            state.pending.push_back(Ok(Delta::Text(text.to_owned())));
                        }
                    }
                }
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("call_0")
                        .to_owned();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_owned();
                    state.tools.insert(index, id.clone());
                    state
                        .pending
                        .push_back(Ok(Delta::ToolCallStart { index, id, name }));
                }
                _ => {}
            }
        }
        "content_block_delta" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let delta = value.get("delta").cloned().unwrap_or(Value::Null);
            match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                "text_delta" => {
                    if let Some(text) = delta.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            state.pending.push_back(Ok(Delta::Text(text.to_owned())));
                        }
                    }
                }
                "input_json_delta" => {
                    if let Some(fragment) = delta.get("partial_json").and_then(Value::as_str) {
                        if !fragment.is_empty() {
                            let id = state
                                .tools
                                .get(&index)
                                .cloned()
                                .unwrap_or_else(|| format!("call_{index}"));
                            state.pending.push_back(Ok(Delta::ToolCallArgs {
                                index,
                                id,
                                fragment: fragment.to_owned(),
                            }));
                        }
                    }
                }
                "thinking_delta" => {
                    if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                        if !text.is_empty() {
                            state
                                .pending
                                .push_back(Ok(Delta::Reasoning(text.to_owned())));
                        }
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(id) = state.tools.get(&index) {
                if state.emitted_end.insert(index) {
                    state.pending.push_back(Ok(Delta::ToolCallEnd {
                        index,
                        id: id.clone(),
                    }));
                }
            }
        }
        "message_delta" => {
            if let Some(usage) = value.get("usage") {
                if let Some(tokens) = usage.get("output_tokens").and_then(Value::as_i64) {
                    state.output_tokens = tokens;
                }
            }
            if let Some(reason) = value
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str)
            {
                state.finish = Some(parse_finish_reason(reason));
            }
            state.pending.push_back(Ok(Delta::Usage(Usage {
                prompt_tokens: state.input_tokens,
                completion_tokens: state.output_tokens,
            })));
        }
        "message_stop" => {
            if !state.done {
                state.done = true;
                state.pending.push_back(Ok(Delta::Done(
                    state
                        .finish
                        .clone()
                        .unwrap_or(FinishReason::Unknown("message_stop".into())),
                )));
            }
        }
        "error" => {
            let message = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("The provider returned an error.");
            let err = if crate::error::is_context_overflow_message(message) {
                CoreError::ContextOverflow
            } else {
                CoreError::message(message.to_owned())
            };
            state.pending.push_back(Err(err));
            state.done = true;
        }
        _ => {}
    }
}

fn json_fallback(body: &str) -> Result<DeltaStream> {
    let value: Value = serde_json::from_str(body)?;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The provider returned an error.");
        return Err(CoreError::message(message.to_owned()));
    }
    let mut deltas = Vec::new();
    if let Some(usage) = value.get("usage").and_then(Usage::from_json) {
        deltas.push(Ok(Delta::Usage(usage)));
    }
    if let Some(blocks) = value.get("content").and_then(Value::as_array) {
        for (index, block) in blocks.iter().enumerate() {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        deltas.push(Ok(Delta::Text(text.to_owned())));
                    }
                }
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("call_0")
                        .to_owned();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_owned();
                    deltas.push(Ok(Delta::ToolCallStart {
                        index,
                        id: id.clone(),
                        name,
                    }));
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    deltas.push(Ok(Delta::ToolCallArgs {
                        index,
                        id: id.clone(),
                        fragment: serde_json::to_string(&input)?,
                    }));
                    deltas.push(Ok(Delta::ToolCallEnd { index, id }));
                }
                _ => {}
            }
        }
    }
    let reason = value
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn");
    deltas.push(Ok(Delta::Done(parse_finish_reason(reason))));
    Ok(Box::pin(futures::stream::iter(deltas)))
}

fn build_body(req: &ChatRequest) -> Result<Value> {
    let mut messages: Vec<Value> = Vec::new();
    for message in &req.messages {
        match message.role {
            ChatRole::System => {
                push_message(
                    &mut messages,
                    "user",
                    vec![json!({"type": "text", "text": message.text_content()})],
                );
            }
            ChatRole::User => {
                let mut blocks = Vec::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text(text) if !text.is_empty() => {
                            blocks.push(json!({"type": "text", "text": text}));
                        }
                        ContentBlock::Image { media_type, data } => {
                            blocks.push(json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": media_type,
                                    "data": data,
                                }
                            }));
                        }
                        _ => {}
                    }
                }
                if !blocks.is_empty() {
                    push_message(&mut messages, "user", blocks);
                }
            }
            ChatRole::Assistant => {
                let mut blocks = Vec::new();
                let text = message.text_content();
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                for block in &message.content {
                    if let ContentBlock::ToolCall {
                        id,
                        name,
                        arguments,
                    } = block
                    {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": arguments,
                        }));
                    }
                }
                if !blocks.is_empty() {
                    push_message(&mut messages, "assistant", blocks);
                }
            }
            ChatRole::Tool => {
                let mut blocks = Vec::new();
                for block in &message.content {
                    if let ContentBlock::ToolResult {
                        call_id,
                        content,
                        is_error,
                        ..
                    } = block
                    {
                        blocks.push(json!({
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": if content.is_empty() { "(no output)" } else { content },
                            "is_error": is_error,
                        }));
                    }
                }
                if !blocks.is_empty() {
                    push_message(&mut messages, "user", blocks);
                }
            }
        }
    }

    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens.max(1),
        "messages": messages,
        "stream": true,
    });
    if let Some(system) = req.system.as_deref().filter(|s| !s.trim().is_empty()) {
        body["system"] = json!(system);
    }
    if let Some(temperature) = req.temperature {
        body["temperature"] = json!(temperature);
    }
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                    })
                })
                .collect(),
        );
        body["tool_choice"] = json!(match req.tool_choice {
            ToolChoice::Auto => json!({"type": "auto"}),
            ToolChoice::None => json!({"type": "none"}),
        });
    }
    Ok(body)
}

fn push_message(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if let Some(last) = messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some(role) {
            if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                content.extend(blocks);
                return;
            }
        }
    }
    messages.push(json!({"role": role, "content": blocks}));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatMessage, ChatRequest, ChatRole, ContentBlock, ToolChoice};
    use crate::models::ProviderKind;

    #[test]
    fn users_with_images_send_anthropic_image_blocks() {
        let request = ChatRequest {
            kind: ProviderKind::Anthropic,
            base_url: "https://example.invalid".into(),
            api_key: None,
            model: "vision-model".into(),
            system: None,
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: vec![
                    ContentBlock::Text("What is this?".into()),
                    ContentBlock::Image {
                        media_type: "image/jpeg".into(),
                        data: "BBBB".into(),
                    },
                ],
            }],
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            max_tokens: 1024,
            temperature: None,
        };
        let body = build_body(&request).unwrap();
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["media_type"], "image/jpeg");
        assert_eq!(content[1]["source"]["data"], "BBBB");
    }
}
