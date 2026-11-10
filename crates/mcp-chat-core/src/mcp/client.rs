use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, Command};
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::llm::sse::SseParser;
use crate::mcp::legacy::{SseTransport, parse_rpc_error};
use crate::mcp::tools::{encode_header_value, extract_header_values, filter_valid_tools};
use crate::mcp::types::{
    CallToolResult, DiscoverResult, ListToolsResult, McpError, Negotiated, ServerCapabilities,
    ServerInfo, ToolInfo, TransportKind,
};
use crate::{APP_NAME, APP_VERSION, CoreError, MCP_LEGACY_VERSION, MCP_PROTOCOL_VERSION, Result};

#[derive(Debug, Clone)]
pub enum RpcError {
    Status {
        status: u16,
        rpc: Option<McpError>,
        body: String,
    },
    Rpc(McpError),
    Unauthorized(String),
    Transport(String),
    Cancelled,
}

impl RpcError {
    pub fn into_core(self) -> CoreError {
        match self {
            Self::Rpc(error) => CoreError::Message(error.humanized()),
            Self::Unauthorized(message) => CoreError::Message(message),
            Self::Cancelled => CoreError::Cancelled,
            Self::Status { status, rpc, body } => {
                if let Some(error) = rpc {
                    CoreError::Message(error.humanized())
                } else {
                    let detail = body.chars().take(300).collect::<String>();
                    CoreError::Message(format!(
                        "MCP server returned HTTP {status}{}",
                        if detail.trim().is_empty() {
                            String::new()
                        } else {
                            format!(": {}", detail.trim())
                        }
                    ))
                }
            }
            Self::Transport(message) => CoreError::Message(message),
        }
    }

    fn is_legacy_candidate(&self) -> bool {
        match self {
            Self::Status { status, rpc, .. } => match rpc {
                Some(error) => {
                    // A modern server uses these codes for header/capability
                    // problems. Anything else on 400/404/405 (including the
                    // common legacy "Missing session ID" -32600) means the
                    // endpoint is not speaking the modern stateless protocol.
                    matches!(*status, 400 | 404 | 405) && !matches!(error.code, -32020 | -32021)
                }
                None => matches!(*status, 400 | 404 | 405),
            },
            Self::Rpc(error) => matches!(error.code, -32601 | -32022),
            _ => false,
        }
    }

    fn is_sse_candidate(&self) -> bool {
        matches!(
            self,
            Self::Status {
                status: 404 | 405,
                ..
            } | Self::Transport(_)
        )
    }
}

#[derive(Debug, Clone)]
pub struct RpcId {
    pub value: String,
}

impl RpcId {
    pub fn json(&self) -> Value {
        Value::String(self.value.clone())
    }
}

pub type ProgressFn = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ProgressEvent {
    pub progress: f64,
    pub total: Option<f64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MrtrRetry {
    pub input_responses: Value,
    pub request_state: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub server_id: String,
    pub server_name: String,
    pub server_slug: String,
    pub url: String,
    pub transport: Option<TransportKind>,
    pub headers: HashMap<String, String>,
    pub bearer: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
}

#[derive(Clone)]
pub struct McpClient {
    kind: ClientKind,
}

#[derive(Clone)]
enum ClientKind {
    Http(Arc<Inner>),
    Stdio(Arc<StdioTransport>),
}

struct Inner {
    http: reqwest::Client,
    url: Url,
    protocol: Mutex<String>,
    era: Era,
    session: Mutex<Option<String>>,
    next: AtomicI64,
    static_headers: HeaderMap,
}

enum Era {
    Modern,
    Legacy,
    Sse(Arc<SseTransport>),
}

pub struct McpConnection {
    pub client: McpClient,
    pub negotiated: Negotiated,
}

impl McpClient {
    pub async fn connect(config: ClientConfig, cancel: CancellationToken) -> Result<McpConnection> {
        if config.transport == Some(TransportKind::Stdio) {
            return connect_stdio(config, cancel).await;
        }
        let url = validate_server_url(&config.url)?;
        let http = crate::llm::provider::http_client()?;
        let headers = static_headers(&config)?;

        // An SSE hint alone is not enough: users often pick "HTTP+SSE" while
        // pointing at a /mcp endpoint that actually speaks Streamable HTTP.
        // Only a /sse URL is treated as the deprecated transport up front.
        if url.path().trim_end_matches('/').ends_with("/sse") {
            return connect_sse(config, url, http, headers, cancel).await;
        }

        if config.transport == Some(TransportKind::LegacyHttp) {
            match connect_legacy(&config, &url, &http, &headers, cancel.clone()).await {
                Ok(connection) => return Ok(connection),
                Err(error) if error.is_sse_candidate() => {
                    return connect_sse(config, url, http, headers, cancel).await;
                }
                Err(error) => return Err(error.into_core()),
            }
        }

        let modern = Inner {
            http: http.clone(),
            url: url.clone(),
            protocol: Mutex::new(MCP_PROTOCOL_VERSION.to_owned()),
            era: Era::Modern,
            session: Mutex::new(None),
            next: AtomicI64::new(1),
            static_headers: headers.clone(),
        };
        let modern_result = modern
            .rpc("server/discover", json!({}), Vec::new(), None, &cancel)
            .await;

        match modern_result {
            Ok(result) => {
                let supported = supported_versions(&result);
                if supported.is_empty() || supported.iter().any(|v| v == MCP_PROTOCOL_VERSION) {
                    let negotiated = negotiated_modern(&result, TransportKind::StreamableHttp);
                    return Ok(McpConnection {
                        client: McpClient {
                            kind: ClientKind::Http(Arc::new(modern)),
                        },
                        negotiated,
                    });
                }
                if supported.iter().any(|v| v == MCP_LEGACY_VERSION) {
                    return connect_legacy(&config, &url, &http, &headers, cancel)
                        .await
                        .map_err(RpcError::into_core);
                }
                Err(CoreError::Message(format!(
                    "This server supports MCP versions {}, but mcp-chat supports {} or {}.",
                    supported.join(", "),
                    MCP_PROTOCOL_VERSION,
                    MCP_LEGACY_VERSION
                )))
            }
            Err(error) => {
                if let RpcError::Rpc(ref mcp) = error {
                    if mcp.code == -32022 {
                        if let Some(supported) = mcp
                            .data
                            .as_ref()
                            .and_then(|d| d.get("supported"))
                            .and_then(Value::as_array)
                        {
                            let supported = supported
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>();
                            if !supported.contains(&MCP_PROTOCOL_VERSION)
                                && supported.contains(&MCP_LEGACY_VERSION)
                            {
                                return connect_legacy(&config, &url, &http, &headers, cancel)
                                    .await
                                    .map_err(RpcError::into_core);
                            }
                        }
                    }
                }
                if !error.is_legacy_candidate() {
                    return Err(error.into_core());
                }
                match connect_legacy(&config, &url, &http, &headers, cancel.clone()).await {
                    Ok(connection) => Ok(connection),
                    Err(error) if error.is_sse_candidate() => {
                        connect_sse(config, url, http, headers, cancel).await
                    }
                    Err(error) => Err(error.into_core()),
                }
            }
        }
    }

    pub fn protocol_version(&self) -> String {
        match &self.kind {
            ClientKind::Http(inner) => inner.protocol.lock().expect("protocol lock").clone(),
            ClientKind::Stdio(transport) => transport.protocol_version(),
        }
    }

    pub fn transport(&self) -> TransportKind {
        match &self.kind {
            ClientKind::Http(inner) => match inner.era {
                Era::Modern => TransportKind::StreamableHttp,
                Era::Legacy => TransportKind::LegacyHttp,
                Era::Sse(_) => TransportKind::Sse,
            },
            ClientKind::Stdio(_) => TransportKind::Stdio,
        }
    }

    pub fn is_closed(&self) -> bool {
        match &self.kind {
            ClientKind::Http(_) => false,
            ClientKind::Stdio(transport) => transport.is_closed(),
        }
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        extra_headers: Vec<(String, String)>,
        progress: Option<ProgressFn>,
        cancel: &CancellationToken,
    ) -> std::result::Result<Value, RpcError> {
        match &self.kind {
            ClientKind::Http(inner) => {
                inner
                    .rpc(method, params, extra_headers, progress, cancel)
                    .await
            }
            ClientKind::Stdio(transport) => {
                let _ = extra_headers;
                transport.request(method, params, progress, cancel).await
            }
        }
    }

    pub async fn list_tools(&self, cancel: &CancellationToken) -> Result<ListToolsResult> {
        let mut tools: Vec<ToolInfo> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut ttl: Option<i64> = None;
        let mut cache_scope: Option<String> = None;
        for _ in 0..50 {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let value = self
                .rpc("tools/list", params, Vec::new(), None, cancel)
                .await
                .map_err(RpcError::into_core)?;
            let page: ListToolsResult = serde_json::from_value(value)?;
            tools.extend(page.tools);
            ttl = match (ttl, page.ttl_ms) {
                (Some(current), Some(next)) => Some(current.min(next)),
                (current, next) => current.or(next),
            };
            cache_scope = cache_scope.or(page.cache_scope);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let mut tools = filter_valid_tools(tools);
        let mut seen = std::collections::HashSet::new();
        tools.retain(|tool| seen.insert(tool.name.clone()));
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            ttl_ms: ttl.map(|value| value.clamp(0, 600_000)),
            cache_scope,
            result_type: Some("complete".into()),
        })
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: &Value,
        tool: Option<&ToolInfo>,
        retry: Option<MrtrRetry>,
        progress: Option<ProgressFn>,
        cancel: &CancellationToken,
    ) -> Result<CallToolResult> {
        let mut params = json!({"name": name, "arguments": arguments});
        if let Some(retry) = retry {
            params["inputResponses"] = retry.input_responses;
            if let Some(state) = retry.request_state {
                params["requestState"] = state;
            }
        }
        let extra = tool
            .map(|tool| extract_header_values(tool, arguments))
            .unwrap_or_default();
        let value = self
            .rpc("tools/call", params, extra, progress, cancel)
            .await
            .map_err(RpcError::into_core)?;
        let result: CallToolResult = serde_json::from_value(value)?;
        match result.result_type() {
            "complete" | "input_required" => Ok(result),
            other => Err(CoreError::Message(format!(
                "The server returned an unknown MCP result type: {other}"
            ))),
        }
    }

    pub async fn notify_tools_changed(&self) -> Result<()> {
        // Only useful for subscriptions/listen; sending an initialized
        // notification is legacy-only and handled at connect time.
        Ok(())
    }
}

struct PendingCall {
    tx: oneshot::Sender<std::result::Result<Value, RpcError>>,
    progress: Option<ProgressFn>,
}

struct StdioTransport {
    label: String,
    stdin: AsyncMutex<Box<dyn AsyncWrite + Unpin + Send>>,
    pending: AsyncMutex<HashMap<String, PendingCall>>,
    protocol: Mutex<String>,
    closed: AtomicBool,
    _child: AsyncMutex<Option<Child>>,
    stderr_tail: Mutex<VecDeque<String>>,
    next: AtomicI64,
}

impl StdioTransport {
    async fn spawn(config: &ClientConfig) -> Result<Arc<Self>> {
        let command = config.command.trim();
        if command.is_empty() {
            return Err(CoreError::invalid(
                "A command is required for a stdio server",
            ));
        }
        let mut child_command = build_stdio_command(command, &config.args);
        child_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = config
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty())
        {
            child_command.current_dir(cwd);
        }
        for (key, value) in &config.env {
            child_command.env(key, value);
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            child_command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = child_command.spawn().map_err(|error| {
            CoreError::Message(format!(
                "Could not start MCP command {command:?}: {error}. Install it or check the command path."
            ))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::Message("MCP process did not expose stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::Message("MCP process did not expose stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CoreError::Message("MCP process did not expose stderr".into()))?;

        let transport = Arc::new(Self {
            label: config.server_slug.clone(),
            stdin: AsyncMutex::new(Box::new(stdin)),
            pending: AsyncMutex::new(HashMap::new()),
            protocol: Mutex::new(MCP_LEGACY_VERSION.to_owned()),
            closed: AtomicBool::new(false),
            _child: AsyncMutex::new(Some(child)),
            stderr_tail: Mutex::new(VecDeque::new()),
            next: AtomicI64::new(1),
        });
        tokio::spawn(read_stdio_stdout(transport.clone(), stdout));
        tokio::spawn(read_stdio_stderr(transport.clone(), stderr));
        Ok(transport)
    }

    #[cfg(test)]
    fn from_io(
        label: impl Into<String>,
        stdout: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        stdin: impl AsyncWrite + Unpin + Send + 'static,
    ) -> Arc<Self> {
        let transport = Arc::new(Self {
            label: label.into(),
            stdin: AsyncMutex::new(Box::new(stdin)),
            pending: AsyncMutex::new(HashMap::new()),
            protocol: Mutex::new(MCP_LEGACY_VERSION.to_owned()),
            closed: AtomicBool::new(false),
            _child: AsyncMutex::new(None),
            stderr_tail: Mutex::new(VecDeque::new()),
            next: AtomicI64::new(1),
        });
        tokio::spawn(read_stdio_stdout(transport.clone(), stdout));
        transport
    }

    fn protocol_version(&self) -> String {
        self.protocol.lock().expect("protocol lock").clone()
    }

    fn set_protocol(&self, protocol: String) {
        *self.protocol.lock().expect("protocol lock") = protocol;
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    async fn request(
        &self,
        method: &str,
        mut params: Value,
        progress: Option<ProgressFn>,
        cancel: &CancellationToken,
    ) -> std::result::Result<Value, RpcError> {
        let id = format!("mcp-{}", self.next.fetch_add(1, Ordering::SeqCst));
        if !params.is_object() {
            params = json!({});
        }
        if progress.is_some() {
            if !params.get("_meta").map(Value::is_object).unwrap_or(false) {
                params["_meta"] = json!({});
            }
            params["_meta"]["progressToken"] = Value::String(id.clone());
        }
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .await
            .insert(id.clone(), PendingCall { tx, progress });
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_message(&body, cancel).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        tokio::select! {
            _ = cancel.cancelled() => {
                self.pending.lock().await.remove(&id);
                Err(RpcError::Cancelled)
            }
            response = rx => match response {
                Ok(result) => result,
                Err(_) => Err(RpcError::Transport(
                    "The MCP process ended before it answered.".into(),
                )),
            }
        }
    }

    async fn notify(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> std::result::Result<(), RpcError> {
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.write_message(&body, cancel).await
    }

    async fn write_message(
        &self,
        value: &Value,
        cancel: &CancellationToken,
    ) -> std::result::Result<(), RpcError> {
        if self.is_closed() {
            return Err(RpcError::Transport(
                "The MCP process is not running.".into(),
            ));
        }
        let mut line = serde_json::to_vec(value).map_err(|error| {
            RpcError::Transport(format!("Could not encode MCP request: {error}"))
        })?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        tokio::select! {
            _ = cancel.cancelled() => Err(RpcError::Cancelled),
            result = async {
                stdin.write_all(&line).await?;
                stdin.flush().await
            } => result.map_err(|error| {
                RpcError::Transport(format!("Could not write to the MCP process: {error}"))
            }),
        }
    }

    async fn handle_line(self: &Arc<Self>, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(server = %self.label, error = %error, "malformed MCP stdio message");
                return;
            }
        };

        if let Some(method) = value.get("method").and_then(Value::as_str) {
            if let Some(id) = value.get("id") {
                self.answer_server_request(id, method).await;
            } else if method == "notifications/progress" {
                self.handle_progress(value.get("params").unwrap_or(&Value::Null))
                    .await;
            } else {
                tracing::debug!(server = %self.label, method, "MCP stdio notification");
            }
            return;
        }

        let Some(id) = value.get("id").and_then(value_id) else {
            tracing::debug!(server = %self.label, "MCP stdio message without method or id");
            return;
        };
        let Some(pending) = self.pending.lock().await.remove(&id) else {
            return;
        };
        let result = if let Some(error) = value.get("error") {
            Err(RpcError::Rpc(McpError {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown MCP error")
                    .to_owned(),
                data: error.get("data").cloned(),
            }))
        } else {
            Ok(value.get("result").cloned().unwrap_or(Value::Null))
        };
        let _ = pending.tx.send(result);
    }

    async fn handle_progress(&self, params: &Value) {
        let Some(token) = params.get("progressToken").and_then(value_id) else {
            return;
        };
        let callback = self
            .pending
            .lock()
            .await
            .get(&token)
            .and_then(|pending| pending.progress.clone());
        if let Some(callback) = callback {
            callback(ProgressEvent {
                progress: params
                    .get("progress")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                total: params.get("total").and_then(Value::as_f64),
                message: params
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }

    async fn answer_server_request(&self, id: &Value, method: &str) {
        // Stdio does not currently provide client-side handlers for
        // server-initiated requests. Answering keeps a server from waiting
        // forever instead of tearing down the process.
        tracing::debug!(server = %self.label, method, "MCP server request is not supported");
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Client method {method} is not supported"),
            },
        });
        let _ = self.write_message(&body, &CancellationToken::new()).await;
    }

    async fn mark_closed(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let tail = self.stderr_tail.lock().expect("stderr lock").clone();
        if !tail.is_empty() {
            tracing::debug!(server = %self.label, stderr = %tail.into_iter().collect::<Vec<_>>().join(" | "), "MCP process exited");
        }
        if let Some(mut child) = self._child.lock().await.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        let mut pending = self.pending.lock().await;
        for (_, call) in pending.drain() {
            let _ = call
                .tx
                .send(Err(RpcError::Transport("The MCP process exited.".into())));
        }
    }
}

fn build_stdio_command(program: &str, args: &[String]) -> Command {
    #[cfg(windows)]
    {
        let lower = program.to_ascii_lowercase();
        let script = lower.ends_with(".cmd") || lower.ends_with(".bat");
        let package_runner = matches!(
            lower.as_str(),
            "npx" | "npm" | "pnpm" | "yarn" | "bunx" | "uvx"
        );
        if script || package_runner {
            let mut command = Command::new("cmd.exe");
            command
                .arg("/C")
                .arg(if lower == "npx" { "npx.cmd" } else { program });
            command.args(args);
            return command;
        }
    }
    let mut command = Command::new(program);
    command.args(args);
    command
}

async fn read_stdio_stdout<R>(transport: Arc<StdioTransport>, stdout: R)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => transport.handle_line(&line).await,
            Ok(None) => break,
            Err(error) => {
                tracing::debug!(server = %transport.label, error = %error, "MCP stdio read failed");
                break;
            }
        }
    }
    transport.mark_closed().await;
}

async fn read_stdio_stderr(transport: Arc<StdioTransport>, stderr: ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(server = %transport.label, "MCP stdio stderr: {line}");
        let mut tail = transport.stderr_tail.lock().expect("stderr lock");
        tail.push_back(line);
        while tail.len() > 20 {
            tail.pop_front();
        }
    }
}

async fn connect_stdio(config: ClientConfig, cancel: CancellationToken) -> Result<McpConnection> {
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
    let transport = StdioTransport::spawn(&config).await?;
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, finish_stdio_connect(transport, &cancel)).await {
        Ok(result) => result,
        Err(_) => Err(CoreError::Message(
            "The local MCP process did not finish its handshake within 60 seconds.".into(),
        )),
    }
}

async fn finish_stdio_connect(
    transport: Arc<StdioTransport>,
    cancel: &CancellationToken,
) -> Result<McpConnection> {
    let result = transport
        .request(
            "initialize",
            json!({
                "protocolVersion": MCP_LEGACY_VERSION,
                "capabilities": {},
                "clientInfo": {"name": APP_NAME, "version": APP_VERSION},
            }),
            None,
            cancel,
        )
        .await
        .map_err(RpcError::into_core)?;
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(MCP_LEGACY_VERSION)
        .to_owned();
    transport.set_protocol(version.clone());
    let _ = transport
        .notify("notifications/initialized", json!({}), cancel)
        .await;
    Ok(McpConnection {
        client: McpClient {
            kind: ClientKind::Stdio(transport),
        },
        negotiated: Negotiated {
            protocol_version: version,
            server_info: parse_server_info(result.get("serverInfo")),
            capabilities: parse_capabilities(result.get("capabilities")),
            instructions: result
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned),
            transport: TransportKind::Stdio,
        },
    })
}

async fn connect_legacy(
    _config: &ClientConfig,
    url: &Url,
    http: &reqwest::Client,
    headers: &HeaderMap,
    cancel: CancellationToken,
) -> std::result::Result<McpConnection, RpcError> {
    let inner = Inner {
        http: http.clone(),
        url: url.clone(),
        protocol: Mutex::new(MCP_LEGACY_VERSION.to_owned()),
        era: Era::Legacy,
        session: Mutex::new(None),
        next: AtomicI64::new(1),
        static_headers: headers.clone(),
    };
    let result = inner
        .rpc(
            "initialize",
            json!({
                "protocolVersion": MCP_LEGACY_VERSION,
                "capabilities": {"elicitation": {}},
                "clientInfo": {"name": APP_NAME, "version": APP_VERSION},
            }),
            Vec::new(),
            None,
            &cancel,
        )
        .await?;
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(MCP_LEGACY_VERSION)
        .to_owned();
    *inner.protocol.lock().expect("protocol lock") = version.clone();
    let _ = inner
        .notify("notifications/initialized", json!({}), &cancel)
        .await;
    let server_info = parse_server_info(result.get("serverInfo"));
    let capabilities = parse_capabilities(result.get("capabilities"));
    let instructions = result
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(McpConnection {
        client: McpClient {
            kind: ClientKind::Http(Arc::new(inner)),
        },
        negotiated: Negotiated {
            protocol_version: version,
            server_info,
            capabilities,
            instructions,
            transport: TransportKind::LegacyHttp,
        },
    })
}

async fn connect_sse(
    _config: ClientConfig,
    url: Url,
    http: reqwest::Client,
    headers: HeaderMap,
    cancel: CancellationToken,
) -> Result<McpConnection> {
    let transport = SseTransport::connect(http.clone(), url.clone(), headers.clone(), &cancel)
        .await
        .map_err(RpcError::into_core)?;
    let transport = Arc::new(transport);
    let inner = Inner {
        http,
        url,
        protocol: Mutex::new(MCP_LEGACY_VERSION.to_owned()),
        era: Era::Sse(transport),
        session: Mutex::new(None),
        next: AtomicI64::new(1),
        static_headers: headers,
    };
    let result = inner
        .rpc(
            "initialize",
            json!({
                "protocolVersion": MCP_LEGACY_VERSION,
                "capabilities": {"elicitation": {}},
                "clientInfo": {"name": APP_NAME, "version": APP_VERSION},
            }),
            Vec::new(),
            None,
            &cancel,
        )
        .await
        .map_err(RpcError::into_core)?;
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(MCP_LEGACY_VERSION)
        .to_owned();
    *inner.protocol.lock().expect("protocol lock") = version.clone();
    let _ = inner
        .notify("notifications/initialized", json!({}), &cancel)
        .await;
    let server_info = parse_server_info(result.get("serverInfo"));
    let capabilities = parse_capabilities(result.get("capabilities"));
    let instructions = result
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(McpConnection {
        client: McpClient {
            kind: ClientKind::Http(Arc::new(inner)),
        },
        negotiated: Negotiated {
            protocol_version: version,
            server_info,
            capabilities,
            instructions,
            transport: TransportKind::Sse,
        },
    })
}

impl Inner {
    async fn rpc(
        &self,
        method: &str,
        params: Value,
        extra_headers: Vec<(String, String)>,
        progress: Option<ProgressFn>,
        cancel: &CancellationToken,
    ) -> std::result::Result<Value, RpcError> {
        let id = RpcId {
            value: format!("mcp-{}", self.next.fetch_add(1, Ordering::SeqCst)),
        };
        let modern = matches!(self.era, Era::Modern);
        let protocol = self.protocol.lock().expect("protocol lock").clone();
        let mut params = if params.is_object() {
            params
        } else {
            json!({})
        };
        if !params.get("_meta").map(Value::is_object).unwrap_or(false) {
            params["_meta"] = json!({});
        }
        if modern {
            params["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!(protocol);
            params["_meta"]["io.modelcontextprotocol/clientInfo"] =
                json!({"name": APP_NAME, "version": APP_VERSION});
            params["_meta"]["io.modelcontextprotocol/clientCapabilities"] =
                json!({"elicitation": {}});
        }
        if progress.is_some() {
            params["_meta"]["progressToken"] = id.json();
        }

        let source_name = params
            .get("name")
            .or_else(|| params.get("uri"))
            .and_then(Value::as_str)
            .map(str::to_owned);

        match &self.era {
            Era::Sse(transport) => {
                let result = transport.request(&id, method, params, cancel).await?;
                call_progress(&progress, &result);
                Ok(result)
            }
            Era::Modern | Era::Legacy => {
                let body = json!({
                    "jsonrpc": "2.0",
                    "id": id.json(),
                    "method": method,
                    "params": params,
                });
                let mut headers = self.static_headers.clone();
                if modern {
                    headers.insert(
                        "MCP-Protocol-Version",
                        HeaderValue::from_str(&protocol).expect("static protocol header"),
                    );
                    headers.insert(
                        "Mcp-Method",
                        HeaderValue::from_str(method)
                            .map_err(|_| RpcError::Transport("Invalid method header".into()))?,
                    );
                    if let Some(name) = source_name.as_deref() {
                        headers.insert(
                            "Mcp-Name",
                            HeaderValue::from_str(&encode_header_value(name)).map_err(|_| {
                                RpcError::Transport("Invalid Mcp-Name header".into())
                            })?,
                        );
                    }
                } else if protocol.as_str() >= "2025-06-18" {
                    headers.insert(
                        "MCP-Protocol-Version",
                        HeaderValue::from_str(&protocol).expect("static protocol header"),
                    );
                }
                if !modern {
                    if let Some(session) = self.session.lock().expect("session lock").as_deref() {
                        headers.insert(
                            "Mcp-Session-Id",
                            HeaderValue::from_str(session)
                                .map_err(|_| RpcError::Transport("Invalid session id".into()))?,
                        );
                    }
                }
                for (name, value) in extra_headers {
                    let name =
                        reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                            RpcError::Transport(format!("Invalid header name {name:?}"))
                        })?;
                    let value = HeaderValue::from_str(&value).map_err(|_| {
                        RpcError::Transport(format!("Invalid value for header {name:?}"))
                    })?;
                    headers.insert(name, value);
                }

                let response = tokio::select! {
                    _ = cancel.cancelled() => return Err(RpcError::Cancelled),
                    response = self
                        .http
                        .post(self.url.clone())
                        .headers(headers)
                        .json(&body)
                        .send() => response.map_err(|err| RpcError::Transport(err.to_string()))?,
                };

                if let Some(session) = response
                    .headers()
                    .get("Mcp-Session-Id")
                    .and_then(|value| value.to_str().ok())
                {
                    *self.session.lock().expect("session lock") = Some(session.to_owned());
                }
                let status = response.status();
                if status == reqwest::StatusCode::UNAUTHORIZED {
                    return Err(RpcError::Unauthorized(
                        "The MCP server rejected the credentials. Reconnect or update the token."
                            .into(),
                    ));
                }
                if !status.is_success() {
                    let body = response.text().await.unwrap_or_default();
                    return Err(RpcError::Status {
                        status: status.as_u16(),
                        rpc: parse_rpc_error(&body),
                        body,
                    });
                }
                let content_type = response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                if content_type.contains("text/event-stream") {
                    read_sse_response(response, &id.value, &progress, cancel).await
                } else {
                    let body = response.text().await.unwrap_or_default();
                    if body.trim().is_empty() {
                        return Err(RpcError::Transport(
                            "The MCP server returned an empty response.".into(),
                        ));
                    }
                    let value: Value = serde_json::from_str(&body).map_err(|err| {
                        RpcError::Transport(format!("Malformed MCP response: {err}"))
                    })?;
                    extract_result(value, &id.value, &progress)?.ok_or_else(|| {
                        RpcError::Transport("The MCP response did not include a result.".into())
                    })
                }
            }
        }
    }

    async fn notify(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> std::result::Result<(), RpcError> {
        match &self.era {
            Era::Sse(transport) => transport.notify(method, params, cancel).await,
            Era::Modern | Era::Legacy => {
                let body = json!({"jsonrpc": "2.0", "method": method, "params": params});
                let mut request = self.http.post(self.url.clone()).json(&body);
                for (name, value) in self.static_headers.iter() {
                    request = request.header(name, value);
                }
                if matches!(self.era, Era::Legacy) {
                    let protocol = self.protocol.lock().expect("protocol lock").clone();
                    if protocol.as_str() >= "2025-06-18" {
                        if let Ok(value) = HeaderValue::from_str(&protocol) {
                            request = request.header("MCP-Protocol-Version", value);
                        }
                    }
                    if let Some(session) = self.session.lock().expect("session lock").as_deref() {
                        if let Ok(value) = HeaderValue::from_str(session) {
                            request = request.header("Mcp-Session-Id", value);
                        }
                    }
                }
                let response = tokio::select! {
                    _ = cancel.cancelled() => return Err(RpcError::Cancelled),
                    response = request.send() => response.map_err(|err| RpcError::Transport(err.to_string()))?,
                };
                if response.status().is_success() {
                    Ok(())
                } else {
                    Err(RpcError::Status {
                        status: response.status().as_u16(),
                        rpc: None,
                        body: String::new(),
                    })
                }
            }
        }
    }
}

async fn read_sse_response(
    response: reqwest::Response,
    expected_id: &str,
    progress: &Option<ProgressFn>,
    cancel: &CancellationToken,
) -> std::result::Result<Value, RpcError> {
    let mut stream = response.bytes_stream();
    let mut parser = SseParser::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(RpcError::Cancelled),
            chunk = stream.next() => chunk,
        };
        match chunk {
            Some(Ok(bytes)) => {
                for event in parser.push(&bytes) {
                    if event.data.trim().is_empty() {
                        continue;
                    }
                    let value: Value = serde_json::from_str(&event.data).map_err(|err| {
                        RpcError::Transport(format!("Malformed MCP event: {err}"))
                    })?;
                    if let Some(result) = extract_result(value, expected_id, progress)? {
                        return Ok(result);
                    }
                }
            }
            Some(Err(err)) => return Err(RpcError::Transport(err.to_string())),
            None => {
                if let Some(event) = parser.finish() {
                    if let Ok(value) = serde_json::from_str::<Value>(&event.data) {
                        if let Some(result) = extract_result(value, expected_id, progress)? {
                            return Ok(result);
                        }
                    }
                }
                return Err(RpcError::Transport(
                    "The MCP stream ended before the response arrived.".into(),
                ));
            }
        }
    }
}

fn extract_result(
    value: Value,
    expected_id: &str,
    progress: &Option<ProgressFn>,
) -> std::result::Result<Option<Value>, RpcError> {
    let items = match value {
        Value::Array(items) => items,
        item => vec![item],
    };
    for item in items {
        let id = item.get("id").and_then(value_id);
        if id.as_deref() == Some(expected_id) {
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
        if let Some(method) = item.get("method").and_then(Value::as_str) {
            if method == "notifications/progress" {
                if let Some(callback) = progress {
                    let params = item.get("params").cloned().unwrap_or(Value::Null);
                    callback(ProgressEvent {
                        progress: params
                            .get("progress")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0),
                        total: params.get("total").and_then(Value::as_f64),
                        message: params
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    });
                }
            } else {
                tracing::debug!(method, "ignoring MCP server notification");
            }
        }
    }
    Ok(None)
}

fn call_progress(progress: &Option<ProgressFn>, value: &Value) {
    if let Some(callback) = progress {
        if value.get("method").and_then(Value::as_str) == Some("notifications/progress") {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            callback(ProgressEvent {
                progress: params
                    .get("progress")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                total: params.get("total").and_then(Value::as_f64),
                message: params
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }
}

fn value_id(value: &Value) -> Option<String> {
    match value {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

fn supported_versions(result: &Value) -> Vec<String> {
    result
        .get("supportedVersions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn negotiated_modern(result: &Value, transport: TransportKind) -> Negotiated {
    let server_info = result
        .get("_meta")
        .and_then(|meta| meta.get("io.modelcontextprotocol/serverInfo"))
        .and_then(|value| parse_server_info(Some(value)));
    let capabilities = parse_capabilities(result.get("capabilities"));
    let instructions = result
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Negotiated {
        protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
        server_info,
        capabilities,
        instructions,
        transport,
    }
}

fn parse_server_info(value: Option<&Value>) -> Option<ServerInfo> {
    value.and_then(|value| serde_json::from_value(value.clone()).ok())
}

fn parse_capabilities(value: Option<&Value>) -> Option<ServerCapabilities> {
    value.and_then(|value| serde_json::from_value(value.clone()).ok())
}

fn static_headers(config: &ClientConfig) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    for (name, value) in &config.headers {
        let lower = name.to_ascii_lowercase();
        if lower == "authorization"
            || lower == "content-type"
            || lower == "accept"
            || lower == "host"
            || lower == "content-length"
            || lower.starts_with("mcp-")
        {
            return Err(CoreError::invalid(format!(
                "Header {name:?} is managed by the app and cannot be overridden"
            )));
        }
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| CoreError::invalid(format!("Invalid header name {name:?}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| CoreError::invalid(format!("Invalid value for header {name:?}")))?;
        headers.insert(name, value);
    }
    if let Some(token) = config.bearer.as_deref() {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| CoreError::invalid("Invalid bearer token"))?;
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    Ok(headers)
}

pub fn validate_server_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).map_err(|_| CoreError::invalid("Invalid server URL"))?;
    match url.scheme() {
        "https" => {}
        "http" => {
            let host = url.host_str().unwrap_or("");
            if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
                return Err(CoreError::invalid(
                    "HTTP is only allowed for localhost development servers",
                ));
            }
        }
        _ => return Err(CoreError::invalid("Server URL must use https://")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CoreError::invalid(
            "Server URL must not contain credentials",
        ));
    }
    if url.fragment().is_some() {
        return Err(CoreError::invalid("Server URL must not contain a fragment"));
    }
    Ok(url)
}

/// Parse the `supported` list from an UnsupportedProtocolVersion error.
pub fn supported_from_error(error: &McpError) -> Vec<String> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("supported"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Exposed for tests and for the MCP screen's capability chips.
pub fn discover_summary(result: &Value) -> DiscoverResult {
    serde_json::from_value(result.clone()).unwrap_or_default()
}

pub fn parse_tool_list(value: &Value) -> Result<ListToolsResult> {
    Ok(serde_json::from_value(value.clone())?)
}

pub fn args_from_json(json: &str) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(json)?;
    let items = value
        .as_array()
        .ok_or_else(|| CoreError::invalid("args must be a JSON array"))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| CoreError::invalid("argument values must be strings"))
        })
        .collect()
}

pub fn env_from_json(json: &str) -> Result<HashMap<String, String>> {
    let value: Value = serde_json::from_str(json)?;
    let object = value
        .as_object()
        .ok_or_else(|| CoreError::invalid("environment must be a JSON object"))?;
    let mut map = HashMap::new();
    for (key, value) in object {
        if key.trim().is_empty() {
            return Err(CoreError::invalid(
                "environment variable names cannot be empty",
            ));
        }
        let value = value
            .as_str()
            .ok_or_else(|| CoreError::invalid("environment values must be strings"))?;
        map.insert(key.clone(), value.to_owned());
    }
    Ok(map)
}

pub fn headers_from_json(json: &str) -> Result<HashMap<String, String>> {
    let value: Value = serde_json::from_str(json)?;
    let object = value
        .as_object()
        .ok_or_else(|| CoreError::invalid("headers must be a JSON object"))?;
    let mut map = HashMap::new();
    for (key, value) in object {
        let value = value
            .as_str()
            .ok_or_else(|| CoreError::invalid("header values must be strings"))?;
        map.insert(key.clone(), value.to_owned());
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, duplex};

    async fn send(writer: &mut (impl AsyncWrite + Unpin), value: Value) {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
        writer.flush().await.unwrap();
    }

    async fn stdio_mock(reader: impl AsyncRead + Unpin, mut writer: impl AsyncWrite + Unpin) {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let value: Value = serde_json::from_str(&line).unwrap();
            let method = value.get("method").and_then(Value::as_str).unwrap_or("");
            let id = value.get("id").cloned();
            match method {
                "initialize" => {
                    send(
                        &mut writer,
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "protocolVersion": MCP_LEGACY_VERSION,
                                "serverInfo": {"name": "stdio-mock", "version": "1.0"},
                                "capabilities": {"tools": {}},
                            }
                        }),
                    )
                    .await;
                }
                "notifications/initialized" => {}
                "tools/list" => {
                    send(
                        &mut writer,
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "tools": [{
                                    "name": "echo",
                                    "description": "Echo text",
                                    "inputSchema": {
                                        "type": "object",
                                        "properties": {"text": {"type": "string"}}
                                    }
                                }]
                            }
                        }),
                    )
                    .await;
                }
                "tools/call" => {
                    if let Some(token) = value.pointer("/params/_meta/progressToken").cloned() {
                        send(
                            &mut writer,
                            json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/progress",
                                "params": {
                                    "progressToken": token,
                                    "progress": 1,
                                    "total": 2,
                                    "message": "half"
                                }
                            }),
                        )
                        .await;
                    }
                    let text = value
                        .pointer("/params/arguments/text")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    send(
                        &mut writer,
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": format!("echo: {text}")}]
                            }
                        }),
                    )
                    .await;
                }
                _ => {
                    send(
                        &mut writer,
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32601, "message": "unknown method"}
                        }),
                    )
                    .await;
                }
            }
        }
    }

    #[tokio::test]
    async fn stdio_handshake_lists_tools_and_reports_progress() {
        let (client, server) = duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client);
        let (server_read, server_write) = tokio::io::split(server);
        tokio::spawn(stdio_mock(server_read, server_write));
        let transport = StdioTransport::from_io("stdio-test", client_read, client_write);
        let connection = finish_stdio_connect(transport, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(connection.negotiated.transport, TransportKind::Stdio);
        assert_eq!(connection.negotiated.protocol_version, MCP_LEGACY_VERSION);
        assert_eq!(
            connection
                .negotiated
                .server_info
                .as_ref()
                .map(|info| info.name.as_str()),
            Some("stdio-mock")
        );

        let tools = connection
            .client
            .list_tools(&CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(tools.tools.len(), 1);
        let echo = &tools.tools[0];
        let progress = Arc::new(Mutex::new(Vec::new()));
        let sink = progress.clone();
        let result = connection
            .client
            .call_tool(
                "echo",
                &json!({"text": "hi"}),
                Some(echo),
                None,
                Some(Arc::new(move |event| {
                    sink.lock().unwrap().push(event);
                })),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.content[0]["text"], "echo: hi");
        assert_eq!(progress.lock().unwrap().len(), 1);
        assert_eq!(progress.lock().unwrap()[0].message.as_deref(), Some("half"));
    }

    #[tokio::test]
    async fn a_missing_stdio_command_is_reported_without_panicking() {
        let error = match McpClient::connect(
            ClientConfig {
                server_id: "test".into(),
                server_name: "Local".into(),
                server_slug: "local".into(),
                url: String::new(),
                transport: Some(TransportKind::Stdio),
                headers: HashMap::new(),
                bearer: None,
                command: "mcp-chat-command-that-does-not-exist".into(),
                args: Vec::new(),
                env: HashMap::new(),
                cwd: None,
            },
            CancellationToken::new(),
        )
        .await
        {
            Ok(_) => panic!("a missing command must fail"),
            Err(error) => error,
        };
        assert!(error.user_message().contains("Could not start MCP command"));
    }

    #[test]
    fn stdio_args_and_env_json_are_validated() {
        assert_eq!(
            args_from_json(r#"["-y","pkg"]"#).unwrap(),
            vec!["-y".to_owned(), "pkg".to_owned()]
        );
        assert!(args_from_json(r#""pkg""#).is_err());
        assert_eq!(
            env_from_json(r#"{"TOKEN":"secret"}"#)
                .unwrap()
                .get("TOKEN")
                .map(String::as_str),
            Some("secret")
        );
        assert!(env_from_json(r#"{"TOKEN":1}"#).is_err());
    }
}
