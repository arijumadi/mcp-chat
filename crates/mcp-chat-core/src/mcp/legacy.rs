//! Deprecated HTTP+SSE transport (2024-11-05) and small legacy helpers.
//!
//! The POST side is a normal endpoint; responses and notifications arrive on a
//! long-lived GET event stream. We keep a pending-request map so `tools/call`
//! and handshake requests can wait for their response.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::llm::sse::SseParser;
use crate::mcp::client::{RpcError, RpcId};
use crate::mcp::types::McpError;

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<std::result::Result<Value, RpcError>>>>>;

pub struct SseTransport {
    http: reqwest::Client,
    pub endpoint: reqwest::Url,
    headers: HeaderMap,
    pending: Pending,
    listener_cancel: CancellationToken,
}

impl SseTransport {
    pub async fn connect(
        http: reqwest::Client,
        url: reqwest::Url,
        headers: HeaderMap,
        cancel: &CancellationToken,
    ) -> std::result::Result<Self, RpcError> {
        let mut get = http.get(url.clone()).header(ACCEPT, "text/event-stream");
        for (name, value) in headers.iter() {
            get = get.header(name, value);
        }
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(RpcError::Cancelled),
            response = get.send() => response.map_err(|err| RpcError::Transport(err.to_string()))?,
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(RpcError::Unauthorized(
                "The server requires authentication.".into(),
            ));
        }
        if !response.status().is_success() {
            return Err(RpcError::Status {
                status: response.status().as_u16(),
                rpc: None,
                body: response.text().await.unwrap_or_default(),
            });
        }

        let mut stream = response.bytes_stream();
        let mut parser = SseParser::new();
        let endpoint_data = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let chunk = tokio::select! {
                    _ = cancel.cancelled() => return Err(RpcError::Cancelled),
                    chunk = stream.next() => chunk,
                };
                match chunk {
                    Some(Ok(bytes)) => {
                        for event in parser.push(&bytes) {
                            if event.event.as_deref() == Some("endpoint") {
                                return Ok(event.data);
                            }
                        }
                    }
                    Some(Err(err)) => return Err(RpcError::Transport(err.to_string())),
                    None => {
                        return Err(RpcError::Transport(
                            "The SSE stream closed before sending an endpoint event.".into(),
                        ));
                    }
                }
            }
        })
        .await
        .map_err(|_| {
            RpcError::Transport("Timed out waiting for the SSE endpoint event.".into())
        })??;

        let endpoint = url
            .join(endpoint_data.trim())
            .map_err(|err| RpcError::Transport(format!("Invalid SSE endpoint: {err}")))?;
        if endpoint.scheme() != url.scheme() || endpoint.host_str() != url.host_str() {
            return Err(RpcError::Transport(
                "The server advertised an SSE endpoint on a different origin.".into(),
            ));
        }

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let listener_cancel = CancellationToken::new();
        let listener_pending = pending.clone();
        let listener_cancel_task = listener_cancel.clone();
        tokio::spawn(async move {
            let mut parser = parser;
            loop {
                let chunk = tokio::select! {
                    _ = listener_cancel_task.cancelled() => break,
                    chunk = stream.next() => chunk,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                match chunk {
                    Ok(bytes) => {
                        for event in parser.push(&bytes) {
                            dispatch(&listener_pending, &event.data, None);
                        }
                    }
                    Err(_) => break,
                }
            }
            let mut map = listener_pending.lock().expect("sse pending lock");
            for (_, sender) in map.drain() {
                let _ = sender.send(Err(RpcError::Transport(
                    "The SSE connection closed before the server responded.".into(),
                )));
            }
        });

        Ok(Self {
            http,
            endpoint,
            headers,
            pending,
            listener_cancel,
        })
    }

    pub async fn request(
        &self,
        id: &RpcId,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> std::result::Result<Value, RpcError> {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("sse pending lock")
            .insert(id.value.clone(), sender);

        let body = json!({"jsonrpc": "2.0", "id": id.json(), "method": method, "params": params});
        let mut post = self.http.post(self.endpoint.clone()).json(&body);
        for (name, value) in self.headers.iter() {
            post = post.header(name, value);
        }
        let response = tokio::select! {
            _ = cancel.cancelled() => {
                self.pending.lock().expect("sse pending lock").remove(&id.value);
                return Err(RpcError::Cancelled);
            }
            response = post.send() => response.map_err(|err| RpcError::Transport(err.to_string()))?,
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(RpcError::Unauthorized(
                "The server requires authentication.".into(),
            ));
        }
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(RpcError::Status {
                status,
                rpc: parse_rpc_error(&body),
                body,
            });
        }
        // Some servers return the response directly on POST even in SSE mode.
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if content_type.contains("application/json") {
            let body = response.text().await.unwrap_or_default();
            if !body.trim().is_empty() {
                if let Some(value) = parse_response(&body, &id.value)? {
                    return Ok(value);
                }
            }
        }

        tokio::select! {
            _ = cancel.cancelled() => {
                self.pending.lock().expect("sse pending lock").remove(&id.value);
                Err(RpcError::Cancelled)
            }
            received = tokio::time::timeout(Duration::from_secs(180), receiver) => {
                match received {
                    Ok(Ok(result)) => result,
                    Ok(Err(_)) => Err(RpcError::Transport("The SSE connection closed.".into())),
                    Err(_) => {
                        self.pending.lock().expect("sse pending lock").remove(&id.value);
                        Err(RpcError::Transport("Timed out waiting for the MCP server.".into()))
                    }
                }
            }
        }
    }

    pub async fn notify(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> std::result::Result<(), RpcError> {
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let mut post = self.http.post(self.endpoint.clone()).json(&body);
        for (name, value) in self.headers.iter() {
            post = post.header(name, value);
        }
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(RpcError::Cancelled),
            response = post.send() => response.map_err(|err| RpcError::Transport(err.to_string()))?,
        };
        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            Err(RpcError::Status {
                status,
                rpc: parse_rpc_error(&body),
                body,
            })
        }
    }
}

impl Drop for SseTransport {
    fn drop(&mut self) {
        self.listener_cancel.cancel();
    }
}

fn dispatch(pending: &Pending, data: &str, _progress: Option<&crate::mcp::client::ProgressFn>) {
    if data.trim().is_empty() {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return;
    };
    match value {
        Value::Array(items) => {
            for item in items {
                dispatch_value(pending, item);
            }
        }
        item => dispatch_value(pending, item),
    }
}

fn dispatch_value(pending: &Pending, value: Value) {
    let Some(id) = value.get("id").and_then(value_id) else {
        return;
    };
    let mut map = pending.lock().expect("sse pending lock");
    let Some(sender) = map.remove(&id) else {
        return;
    };
    if let Some(error) = value.get("error") {
        let mcp = McpError {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown MCP error")
                .to_owned(),
            data: error.get("data").cloned(),
        };
        let _ = sender.send(Err(RpcError::Rpc(mcp)));
    } else {
        let _ = sender.send(Ok(value.get("result").cloned().unwrap_or(Value::Null)));
    }
}

fn value_id(value: &Value) -> Option<String> {
    match value {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

pub fn parse_rpc_error(body: &str) -> Option<McpError> {
    let value: Value = serde_json::from_str(body).ok()?;
    let error = value.get("error")?;
    Some(McpError {
        code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Unknown MCP error")
            .to_owned(),
        data: error.get("data").cloned(),
    })
}

pub fn parse_response(
    body: &str,
    expected_id: &str,
) -> std::result::Result<Option<Value>, RpcError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|err| RpcError::Transport(format!("Malformed MCP response: {err}")))?;
    let items = match value {
        Value::Array(items) => items,
        item => vec![item],
    };
    for item in items {
        if item.get("id").and_then(value_id).as_deref() != Some(expected_id) {
            continue;
        }
        if let Some(error) = item.get("error") {
            return Err(RpcError::Rpc(McpError {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown MCP error")
                    .to_owned(),
                data: error.get("data").cloned(),
            }));
        }
        return Ok(Some(item.get("result").cloned().unwrap_or(Value::Null)));
    }
    Ok(None)
}

pub fn header_map(pairs: &[(String, String)]) -> std::result::Result<HeaderMap, RpcError> {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| RpcError::Transport(format!("Invalid header name {name:?}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| RpcError::Transport(format!("Invalid value for header {name:?}")))?;
        headers.insert(name, value);
    }
    Ok(headers)
}
