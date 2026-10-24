use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

#[derive(Clone, Default)]
pub struct ProviderState {
    pub requests: Arc<Mutex<Vec<Value>>>,
    pub counts: Arc<Mutex<HashMap<String, u32>>>,
}

pub struct MockProvider {
    pub url: String,
    pub state: ProviderState,
    pub addr: std::net::SocketAddr,
}

impl MockProvider {
    pub fn requests(&self) -> Vec<Value> {
        self.state.requests.lock().unwrap().clone()
    }
}

pub async fn start() -> MockProvider {
    start_at(0).await
}

pub async fn start_at(port: u16) -> MockProvider {
    let state = ProviderState::default();
    let app = Router::new()
        .route("/v1/chat/completions", post(openai))
        .route("/v1/messages", post(anthropic))
        .route("/v1/models", get(models))
        .route("/requests", get(list_requests))
        .route("/health", get(|| async { "ok" }))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    MockProvider {
        url: format!("http://127.0.0.1:{}/v1", addr.port()),
        state,
        addr,
    }
}

async fn models(State(_state): State<ProviderState>) -> Json<Value> {
    Json(json!({
        "data": [
            {"id": "mock-text"},
            {"id": "mock-tool"},
            {"id": "mock-parallel"},
            {"id": "mock-badargs"},
            {"id": "mock-unknown"},
            {"id": "mock-length"},
            {"id": "mock-cut"},
            {"id": "mock-overflow"},
            {"id": "mock-weird-finish"},
            {"id": "mock-elicit"},
            {"id": "mock-big"},
            {"id": "mock-fail"},
            {"id": "mock-slow"},
            {"id": "mock-header"},
            {"id": "mock-always-tool"},
            {"id": "mock-badjson"},
            {"id": "mock-flaky"},
            {"id": "mock-sleep"},
            {"id": "mock-lazy"},
            {"id": "mock-batch"}
        ]
    }))
}

async fn list_requests(State(state): State<ProviderState>) -> Json<Value> {
    Json(json!(state.requests.lock().unwrap().clone()))
}

fn scenario(body: &Value) -> String {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("mock-text");
    model.strip_prefix("mock-").unwrap_or("text").to_owned()
}

fn bump(state: &ProviderState, key: &str) -> u32 {
    let mut counts = state.counts.lock().unwrap();
    let value = counts.entry(key.to_owned()).or_insert(0);
    *value += 1;
    *value
}

fn has_tool_result(messages: &Value) -> bool {
    messages
        .as_array()
        .map(|items| {
            items.iter().any(|message| {
                message.get("role").and_then(Value::as_str) == Some("tool")
                    || message
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|blocks| {
                            blocks.iter().any(|block| {
                                block.get("type").and_then(Value::as_str) == Some("tool_result")
                            })
                        })
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn last_user_text(messages: &Value) -> String {
    messages
        .as_array()
        .and_then(|items| {
            items.iter().rev().find(|m| {
                m.get("role").and_then(Value::as_str) == Some("user")
                    && m.get("content").map(|c| c.is_string()).unwrap_or(false)
            })
        })
        .and_then(|m| m.get("content").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned()
}

fn tool_name(tools: Option<&Value>, remote: &str) -> String {
    tools
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find_map(|tool| {
                let name = tool
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .or_else(|| tool.get("name"))
                    .and_then(Value::as_str)?;
                if name.ends_with(&format!("__{remote}")) {
                    Some(name.to_owned())
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| remote.to_owned())
}

fn sse(body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE.as_str(), "text/event-stream"),
            ("x-accel-buffering", "no"),
        ],
        body,
    )
        .into_response()
}

fn chunk(value: Value) -> String {
    format!("data: {value}\n\n")
}

fn text_response(text: &str) -> Response {
    let mut body = String::new();
    for piece in text.split_inclusive(' ') {
        body.push_str(&chunk(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {"content": piece}, "finish_reason": null}]
        })));
    }
    body.push_str(&chunk(json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    })));
    body.push_str("data: [DONE]\n\n");
    sse(body)
}

fn tool_call_frames(calls: &[(usize, &str, &str, &str)]) -> String {
    let mut body = String::new();
    for (index, id, name, args) in calls {
        let (first, second) = split_args(args);
        body.push_str(&chunk(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {"tool_calls": [{
                "index": index, "id": id, "type": "function",
                "function": {"name": name, "arguments": first}
            }]}, "finish_reason": null}]
        })));
        if !second.is_empty() {
            body.push_str(&chunk(json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": index, "function": {"arguments": second}
                }]}, "finish_reason": null}]
            })));
        }
    }
    body.push_str(&chunk(json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    })));
    body.push_str("data: [DONE]\n\n");
    body
}

fn split_args(args: &str) -> (&str, &str) {
    if args.len() < 12 {
        return (args, "");
    }
    let cut = args.len() / 2;
    args.split_at(cut)
}

async fn openai(State(state): State<ProviderState>, Json(body): Json<Value>) -> Response {
    state.requests.lock().unwrap().push(body.clone());
    let scenario = scenario(&body);
    let messages = body.get("messages").cloned().unwrap_or_else(|| json!([]));
    let tools = body.get("tools").cloned();
    let system = body
        .get("system")
        .and_then(Value::as_str)
        .or_else(|| {
            messages
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
                })
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
        })
        .unwrap_or("");
    let tools_done = has_tool_result(&messages);

    match scenario.as_str() {
        "text" => text_response("Hello from the mock provider."),
        "flaky" => {
            let count = bump(&state, "flaky");
            if count <= 2 {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": {"message": "temporary mock failure"}})),
                )
                    .into_response();
            }
            text_response("Recovered after retry.")
        }
        "batch" => {
            if tools_done {
                return text_response("Batch complete.");
            }
            let calls = serde_json::json!({
                "calls": [
                    {"server": "mock", "tool": "echo", "arguments": {"text": "batch"}},
                    {"server": "mock", "tool": "add", "arguments": {"a": 2, "b": 3}}
                ]
            });
            sse(tool_call_frames(&[(
                0,
                "call_batch",
                "mcp_execute",
                &calls.to_string(),
            )]))
        }
        "lazy" => {
            let tool_results = messages
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter(|message| {
                            message.get("role").and_then(Value::as_str) == Some("tool")
                        })
                        .count()
                })
                .unwrap_or(0);
            match tool_results {
                0 => sse(tool_call_frames(&[(
                    0,
                    "call_schema",
                    "mcp_get_tool_schema",
                    r#"{"server":"mock","tool":"echo"}"#,
                )])),
                1 => sse(tool_call_frames(&[(
                    0,
                    "call_meta",
                    "mcp_call_tool",
                    r#"{"server":"mock","tool":"echo","arguments":{"text":"lazy"}}"#,
                )])),
                _ => text_response("Lazy tool complete."),
            }
        }
        "sleep" => {
            if tools_done {
                return text_response("Sleep finished.");
            }
            let name = tool_name(tools.as_ref(), "sleep");
            sse(tool_call_frames(&[(
                0,
                "call_sleep",
                &name,
                r#"{"seconds":5}"#,
            )]))
        }
        "always-tool" => {
            let name = tool_name(tools.as_ref(), "echo");
            sse(tool_call_frames(&[(
                0,
                "call_loop",
                &name,
                r#"{"text":"loop"}"#,
            )]))
        }
        "badjson" => {
            if tools_done {
                return text_response("Recovered from bad JSON.");
            }
            let name = tool_name(tools.as_ref(), "echo");
            sse(tool_call_frames(&[(0, "call_badjson", &name, "{")]))
        }
        "tool" | "reject" | "header" | "big" | "fail" | "slow" | "elicit" => {
            if tools_done {
                return text_response("Tool turn complete.");
            }
            let remote = match scenario.as_str() {
                "header" => "header_echo",
                "big" => "big",
                "fail" => "fail",
                "slow" => "slow_progress",
                "elicit" => "elicit",
                _ => "echo",
            };
            let name = tool_name(tools.as_ref(), remote);
            let args = match scenario.as_str() {
                "header" => json!({"region": "us-west1"}),
                "big" => json!({"n": 50000}),
                "slow" => json!({"seconds": 1}),
                "elicit" => json!({"question": "What is your name?"}),
                "fail" => json!({}),
                _ => json!({"text": "hello"}),
            };
            sse(tool_call_frames(&[(0, "call_1", &name, &args.to_string())]))
        }
        "parallel" => {
            if tools_done {
                return text_response("Parallel tools complete.");
            }
            let echo = tool_name(tools.as_ref(), "echo");
            let add = tool_name(tools.as_ref(), "add");
            sse(tool_call_frames(&[
                (0, "call_a", &echo, r#"{"text":"one"}"#),
                (1, "call_b", &add, r#"{"a":2,"b":3}"#),
            ]))
        }
        "unknown" => {
            if tools_done {
                return text_response("Recovered from unknown tool.");
            }
            sse(tool_call_frames(&[(0, "call_x", "does_not_exist", "{}")]))
        }
        "badargs" => {
            if tools_done {
                return text_response("Recovered from invalid args.");
            }
            let echo = tool_name(tools.as_ref(), "echo");
            sse(tool_call_frames(&[(
                0,
                "call_bad",
                &echo,
                r#"{"text":42}"#,
            )]))
        }
        "length" => {
            if last_user_text(&messages).contains("output token limit") {
                return text_response("Second piece.");
            }
            let mut body = String::new();
            body.push_str(&chunk(json!({
                "choices": [{"index": 0, "delta": {"content": "First piece "}, "finish_reason": null}]
            })));
            body.push_str(&chunk(json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "length"}]
            })));
            sse(body)
        }
        "cut" => {
            let count = bump(&state, "cut");
            if count == 1 {
                let mut body = String::new();
                body.push_str(&chunk(json!({
                    "choices": [{"index": 0, "delta": {"content": "Partial text "}, "finish_reason": null}]
                })));
                body.push_str(&chunk(json!({
                    "error": {"message": "mock connection cut", "type": "server_error"}
                })));
                return sse(body);
            }
            text_response("Continued after interruption.")
        }
        "overflow" => {
            if system.contains("conversation compactor") {
                return text_response("Summary of earlier messages.");
            }
            let count = bump(&state, "overflow-main");
            if count == 1 {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": {
                            "message": "This model's maximum context length is 100 tokens.",
                            "type": "invalid_request_error",
                            "code": "context_length_exceeded"
                        }
                    })),
                )
                    .into_response();
            }
            text_response("After compaction.")
        }
        "weird-finish" => {
            let mut body = String::new();
            body.push_str(&chunk(json!({
                "choices": [{"index": 0, "delta": {"content": "Odd finish."}, "finish_reason": null}]
            })));
            body.push_str(&chunk(json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "banana"}]
            })));
            sse(body)
        }
        _ => text_response("Unknown mock scenario."),
    }
}

async fn anthropic(State(state): State<ProviderState>, Json(body): Json<Value>) -> Response {
    state.requests.lock().unwrap().push(body.clone());
    let messages = body.get("messages").cloned().unwrap_or_else(|| json!([]));
    let tools = body.get("tools").cloned();
    let scenario = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("mock-anthropic-text");
    let done = has_tool_result(&messages);
    let mut frames = String::new();
    frames.push_str(&event(
        "message_start",
        json!({
            "type": "message_start",
            "message": {"id": "msg_mock", "usage": {"input_tokens": 12}}
        }),
    ));
    if scenario.contains("tool") && !done {
        let name = tool_name(tools.as_ref(), "echo");
        frames.push_str(&event(
            "content_block_start",
            json!({
                "type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": name, "input": {}}
            }),
        ));
        let args = r#"{"text":"anthropic"}"#;
        let (a, b) = split_args(args);
        frames.push_str(&event(
            "content_block_delta",
            json!({
                "type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": a}
            }),
        ));
        frames.push_str(&event(
            "content_block_delta",
            json!({
                "type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": b}
            }),
        ));
        frames.push_str(&event(
            "content_block_stop",
            json!({
                "type": "content_block_stop", "index": 0
            }),
        ));
        frames.push_str(&event("message_delta", json!({
            "type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 8}
        })));
    } else {
        let text = if done {
            "Anthropic tool complete."
        } else {
            "Hello from Anthropic mock."
        };
        frames.push_str(&event(
            "content_block_start",
            json!({
                "type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
        ));
        for piece in text.split_inclusive(' ') {
            frames.push_str(&event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta", "index": 0,
                    "delta": {"type": "text_delta", "text": piece}
                }),
            ));
        }
        frames.push_str(&event(
            "content_block_stop",
            json!({
                "type": "content_block_stop", "index": 0
            }),
        ));
        frames.push_str(&event("message_delta", json!({
            "type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 8}
        })));
    }
    frames.push_str(&event("message_stop", json!({"type": "message_stop"})));
    (
        [(header::CONTENT_TYPE.as_str(), "text/event-stream")],
        frames,
    )
        .into_response()
}

fn event(name: &str, value: Value) -> String {
    format!("event: {name}\ndata: {value}\n\n")
}
