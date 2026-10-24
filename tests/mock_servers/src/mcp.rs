use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpMode {
    Modern,
    Legacy,
    Sse,
}

#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    pub headers: HashMap<String, String>,
    pub body: Value,
}

#[derive(Clone)]
pub struct McpState {
    pub mode: McpMode,
    pub requests: Arc<Mutex<Vec<CapturedRequest>>>,
    pub sse: Arc<Mutex<Option<broadcast::Sender<String>>>>,
    pub required_api_key: Option<String>,
}

pub struct MockMcp {
    pub url: String,
    pub state: McpState,
    pub addr: std::net::SocketAddr,
}

impl MockMcp {
    pub fn requests(&self) -> Vec<CapturedRequest> {
        self.state.requests.lock().unwrap().clone()
    }
}

pub async fn start(mode: McpMode) -> MockMcp {
    start_at(mode, 0).await
}

pub async fn start_with_api_key(mode: McpMode, api_key: &str) -> MockMcp {
    start_at_with_key(mode, 0, Some(api_key.to_owned())).await
}

pub async fn start_at(mode: McpMode, port: u16) -> MockMcp {
    start_at_with_key(mode, port, None).await
}

async fn start_at_with_key(mode: McpMode, port: u16, required_api_key: Option<String>) -> MockMcp {
    let state = McpState {
        mode,
        requests: Arc::new(Mutex::new(Vec::new())),
        sse: Arc::new(Mutex::new(None)),
        required_api_key,
    };
    let app = Router::new()
        .route("/mcp", post(mcp_post))
        .route("/sse", get(sse_get))
        .route("/messages", post(sse_messages))
        .route("/health", get(|| async { "ok" }))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let path = match mode {
        McpMode::Sse => "sse",
        _ => "mcp",
    };
    MockMcp {
        url: format!("http://127.0.0.1:{}/{path}", addr.port()),
        state,
        addr,
    }
}

async fn mcp_post(
    State(state): State<McpState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let captured_headers = headers
        .iter()
        .filter_map(|(name, value)| {
            Some((
                name.as_str().to_ascii_lowercase(),
                value.to_str().ok()?.to_owned(),
            ))
        })
        .collect::<HashMap<_, _>>();
    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    state.requests.lock().unwrap().push(CapturedRequest {
        method: method.clone(),
        headers: captured_headers.clone(),
        body: body.clone(),
    });

    match state.mode {
        McpMode::Modern => modern_post(&state, &headers, body).await,
        McpMode::Legacy => {
            if method == "server/discover" {
                let id = body.get("id").cloned().unwrap_or(Value::Null);
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json_rpc_error(
                        &id,
                        -32600,
                        "Bad Request: Missing session ID",
                    )),
                )
                    .into_response();
            }
            legacy_post(&state, &headers, body, false).await
        }
        McpMode::Sse => (StatusCode::METHOD_NOT_ALLOWED, "Use /sse").into_response(),
    }
}

fn json_rpc(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn json_rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

async fn modern_post(state: &McpState, headers: &HeaderMap, body: Value) -> Response {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let protocol = headers
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if protocol != "2026-07-28" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_rpc_error(&id, -32020, "MCP-Protocol-Version mismatch")),
        )
            .into_response();
    }
    if headers
        .get("mcp-method")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        != method
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_rpc_error(&id, -32020, "Mcp-Method header mismatch")),
        )
            .into_response();
    }
    if method == "tools/call" {
        let name = body
            .get("params")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if headers
            .get("mcp-name")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            != name
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(json_rpc_error(&id, -32020, "Mcp-Name header mismatch")),
            )
                .into_response();
        }
    }
    let meta = body.get("params").and_then(|p| p.get("_meta"));
    let valid_meta = meta
        .and_then(|m| m.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
        == Some("2026-07-28");
    if valid_meta
        && meta
            .and_then(|m| m.get("io.modelcontextprotocol/clientCapabilities"))
            .is_none()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_rpc_error(&id, -32602, "clientCapabilities missing")),
        )
            .into_response();
    }
    if !valid_meta {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_rpc_error(
                &id,
                -32602,
                "request _meta missing protocol version",
            )),
        )
            .into_response();
    }

    match method {
        "server/discover" => Json(json_rpc(&id, discover_result())).into_response(),
        "tools/list" => Json(json_rpc(&id, tools_result())).into_response(),
        "tools/call" => call_tool(state, &id, &body, true).await,
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        _ => (
            StatusCode::NOT_FOUND,
            Json(json_rpc_error(&id, -32601, "Method not found")),
        )
            .into_response(),
    }
}

async fn legacy_post(state: &McpState, headers: &HeaderMap, body: Value, sse: bool) -> Response {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    if method != "initialize" {
        let session = headers
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if session != "test-session" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json_rpc_error(&id, -32600, "missing session")),
            )
                .into_response();
        }
    }
    match method {
        "initialize" => {
            let value = json_rpc(
                &id,
                json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "mock-legacy", "version": "1.0.0"},
                    "instructions": "Legacy mock server"
                }),
            );
            if sse {
                StatusCode::ACCEPTED.into_response()
            } else {
                let mut response = Json(value).into_response();
                response
                    .headers_mut()
                    .insert("Mcp-Session-Id", "test-session".parse().unwrap());
                response
            }
        }
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/list" => Json(json_rpc(&id, tools_result())).into_response(),
        "tools/call" => call_tool(state, &id, &body, false).await,
        _ => (
            StatusCode::NOT_FOUND,
            Json(json_rpc_error(&id, -32601, "Method not found")),
        )
            .into_response(),
    }
}

fn discover_result() -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": {"tools": {}},
        "instructions": "Mock MCP server for mcp-chat tests",
        "_meta": {
            "io.modelcontextprotocol/serverInfo": {"name": "mock-modern", "version": "1.0.0"}
        },
        "ttlMs": 60000,
        "cacheScope": "private"
    })
}

fn tools_result() -> Value {
    json!({
        "resultType": "complete",
        "tools": tool_defs(),
        "ttlMs": 60000,
        "cacheScope": "private"
    })
}

pub fn tool_defs() -> Value {
    json!([
        {
            "name": "echo",
            "description": "Echo text back",
            "inputSchema": {
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "add",
            "description": "Add two numbers",
            "inputSchema": {
                "type": "object",
                "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
                "required": ["a", "b"],
                "additionalProperties": false
            }
        },
        {
            "name": "slow_progress",
            "description": "Sleep and emit progress notifications",
            "inputSchema": {
                "type": "object",
                "properties": {"seconds": {"type": "integer", "minimum": 0}},
                "required": ["seconds"],
                "additionalProperties": false
            }
        },
        {
            "name": "fail",
            "description": "Always fails with a JSON-RPC error",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "elicit",
            "description": "Ask the user a question through MRTR",
            "inputSchema": {
                "type": "object",
                "properties": {"question": {"type": "string"}},
                "required": ["question"],
                "additionalProperties": false
            }
        },
        {
            "name": "sleep",
            "description": "Sleep for a number of seconds (cancellation test)",
            "inputSchema": {
                "type": "object",
                "properties": {"seconds": {"type": "integer", "minimum": 0}},
                "required": ["seconds"],
                "additionalProperties": false
            }
        },
        {
            "name": "big",
            "description": "Return a large payload",
            "inputSchema": {
                "type": "object",
                "properties": {"n": {"type": "integer", "minimum": 1}},
                "required": ["n"],
                "additionalProperties": false
            }
        },
        {
            "name": "header_echo",
            "description": "Echo a value mirrored in Mcp-Param-Region",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "region": {"type": "string", "x-mcp-header": "Region"}
                },
                "required": ["region"],
                "additionalProperties": false
            }
        }
    ])
}

async fn call_tool(_state: &McpState, id: &Value, body: &Value, modern: bool) -> Response {
    let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = |value: Value| {
        let mut result = value;
        if modern {
            result["resultType"] = json!("complete");
        }
        result
    };
    match name {
        "echo" => {
            let text = args.get("text").and_then(Value::as_str).unwrap_or("");
            Json(json_rpc(
                id,
                result(json!({
                    "content": [{"type": "text", "text": format!("echo: {text}")}]
                })),
            ))
            .into_response()
        }
        "add" => {
            let a = args.get("a").and_then(Value::as_f64).unwrap_or(0.0);
            let b = args.get("b").and_then(Value::as_f64).unwrap_or(0.0);
            Json(json_rpc(
                id,
                result(json!({
                    "content": [{"type": "text", "text": format!("{}", a + b)}],
                    "structuredContent": {"sum": a + b}
                })),
            ))
            .into_response()
        }
        "slow_progress" => {
            let seconds = args.get("seconds").and_then(Value::as_u64).unwrap_or(0);
            let token = params
                .get("_meta")
                .and_then(|meta| meta.get("progressToken"))
                .cloned()
                .unwrap_or(Value::Null);
            let mut frames = String::new();
            for i in 0..3 {
                frames.push_str(&format!(
                    "data: {}\n\n",
                    json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": {"progressToken": token, "progress": (i + 1) as f64, "total": 3.0, "message": format!("step {}", i + 1)}
                    })
                ));
            }
            frames.push_str(&format!(
                "data: {}\n\n",
                json_rpc(
                    id,
                    result(json!({
                        "content": [{"type": "text", "text": format!("slept {seconds}s")}]
                    }))
                )
            ));
            sse_response(frames)
        }
        "fail" => (
            StatusCode::OK,
            Json(json_rpc_error(id, -32603, "mock failure")),
        )
            .into_response(),
        "elicit" => {
            let question = args
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or("What is your name?");
            if let Some(responses) = params.get("inputResponses") {
                let answer = responses
                    .get("answer")
                    .and_then(|value| value.get("content"))
                    .and_then(|content| content.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("(none)");
                Json(json_rpc(
                    id,
                    result(json!({
                        "content": [{"type": "text", "text": format!("answer: {answer}")}]
                    })),
                ))
                .into_response()
            } else {
                let mut value = json!({
                    "resultType": "input_required",
                    "inputRequests": {
                        "answer": {
                            "method": "elicitation/create",
                            "params": {
                                "mode": "form",
                                "message": question,
                                "requestedSchema": {
                                    "type": "object",
                                    "properties": {"name": {"type": "string", "title": "Name"}},
                                    "required": ["name"]
                                }
                            }
                        }
                    },
                    "requestState": "mock-state-1"
                });
                if !modern {
                    value.as_object_mut().unwrap().remove("resultType");
                }
                Json(json_rpc(id, value)).into_response()
            }
        }
        "sleep" => {
            let seconds = args.get("seconds").and_then(Value::as_u64).unwrap_or(1);
            tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
            Json(json_rpc(
                id,
                result(json!({
                    "content": [{"type": "text", "text": "slept"}]
                })),
            ))
            .into_response()
        }
        "big" => {
            let n = args.get("n").and_then(Value::as_u64).unwrap_or(50_000) as usize;
            Json(json_rpc(
                id,
                result(json!({
                    "content": [{"type": "text", "text": "x".repeat(n)}]
                })),
            ))
            .into_response()
        }
        "header_echo" => Json(json_rpc(
            id,
            result(json!({
            "content": [{"type": "text", "text": "header ok"}]})),
        ))
        .into_response(),
        _ => (
            StatusCode::OK,
            Json(json_rpc_error(id, -32602, "Unknown tool")),
        )
            .into_response(),
    }
}

fn sse_response(body: String) -> Response {
    (
        [
            ("content-type", "text/event-stream"),
            ("x-accel-buffering", "no"),
        ],
        body,
    )
        .into_response()
}

async fn sse_get(State(state): State<McpState>) -> Response {
    let (sender, receiver) = broadcast::channel::<String>(32);
    *state.sse.lock().unwrap() = Some(sender);
    let first = futures::stream::once(async move {
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "event: endpoint\ndata: /messages?sessionId=test\n\n",
        ))
    });
    let rest = futures::stream::unfold(receiver, |mut receiver| async move {
        match receiver.recv().await {
            Ok(message) => Some((
                Ok::<Bytes, std::io::Error>(Bytes::from(format!("data: {message}\n\n"))),
                receiver,
            )),
            Err(_) => None,
        }
    });
    let stream = first.chain(rest);
    (
        [
            ("content-type", "text/event-stream"),
            ("x-accel-buffering", "no"),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

async fn sse_messages(
    State(state): State<McpState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let session = query.get("sessionId").map(String::as_str).unwrap_or("");
    if session != "test" {
        return (StatusCode::BAD_REQUEST, "bad session").into_response();
    }
    let captured_headers = headers
        .iter()
        .filter_map(|(name, value)| {
            Some((
                name.as_str().to_ascii_lowercase(),
                value.to_str().ok()?.to_owned(),
            ))
        })
        .collect::<HashMap<_, _>>();
    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    state.requests.lock().unwrap().push(CapturedRequest {
        method: method.clone(),
        headers: captured_headers,
        body: body.clone(),
    });
    let response = legacy_body_value(&state, &body).await;
    if let Some(value) = response {
        if let Some(sender) = state.sse.lock().unwrap().as_ref() {
            let _ = sender.send(value.to_string());
        }
    }
    StatusCode::ACCEPTED.into_response()
}

async fn legacy_body_value(state: &McpState, body: &Value) -> Option<Value> {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => Some(json_rpc(
            &id,
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-sse", "version": "1.0.0"}
            }),
        )),
        "notifications/initialized" => None,
        "tools/list" => Some(json_rpc(&id, tools_result())),
        "tools/call" => {
            let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match name {
                "echo" => Some(json_rpc(
                    &id,
                    json!({
                        "content": [{"type": "text", "text": format!("echo: {}", args.get("text").and_then(Value::as_str).unwrap_or(""))}]
                    }),
                )),
                "slow_progress" => {
                    if let Some(sender) = state.sse.lock().unwrap().as_ref() {
                        let _ = sender.send(
                            json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/progress",
                                "params": {"progressToken": "x", "progress": 1, "total": 1}
                            })
                            .to_string(),
                        );
                    }
                    Some(json_rpc(
                        &id,
                        json!({"content": [{"type": "text", "text": "sse done"}]}),
                    ))
                }
                _ => Some(json_rpc_error(&id, -32602, "Unknown tool")),
            }
        }
        _ => Some(json_rpc_error(&id, -32601, "Method not found")),
    }
}
