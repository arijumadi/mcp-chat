use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mcp_chat_core::mcp::ProgressEvent;
use mcp_chat_core::mcp::client::{ClientConfig, McpClient, MrtrRetry};
use mcp_chat_core::mcp::types::TransportKind;
use mcp_mock::mcp::{self, McpMode};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn config(url: &str, transport: Option<TransportKind>) -> ClientConfig {
    ClientConfig {
        server_id: "mock".into(),
        server_name: "Mock".into(),
        server_slug: "mock".into(),
        url: url.into(),
        transport,
        headers: HashMap::new(),
        bearer: None,
        command: String::new(),
        args: Vec::new(),
        env: HashMap::new(),
        cwd: None,
    }
}

#[tokio::test]
async fn modern_discover_and_tools_call() {
    let mock = mcp::start(McpMode::Modern).await;
    let connection = McpClient::connect(config(&mock.url, None), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(connection.negotiated.protocol_version, "2026-07-28");
    assert_eq!(
        connection
            .negotiated
            .server_info
            .as_ref()
            .map(|s| s.name.as_str()),
        Some("mock-modern")
    );
    let tools = connection
        .client
        .list_tools(&CancellationToken::new())
        .await
        .unwrap();
    assert!(tools.tools.iter().any(|tool| tool.name == "echo"));
    let echo = tools.tools.iter().find(|tool| tool.name == "echo").unwrap();
    let result = connection
        .client
        .call_tool(
            "echo",
            &json!({"text":"hi"}),
            Some(echo),
            None,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    assert_eq!(result.content[0]["text"], "echo: hi");

    let requests = mock.requests();
    let call = requests
        .iter()
        .find(|request| request.method == "tools/call")
        .expect("tools/call captured");
    assert_eq!(
        call.headers.get("mcp-method").map(String::as_str),
        Some("tools/call")
    );
    assert_eq!(
        call.headers.get("mcp-name").map(String::as_str),
        Some("echo")
    );
    assert_eq!(
        call.headers.get("mcp-protocol-version").map(String::as_str),
        Some("2026-07-28")
    );
    assert_eq!(
        call.body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        "2026-07-28"
    );
}

#[tokio::test]
async fn progress_and_mrtr_round_trip() {
    let mock = mcp::start(McpMode::Modern).await;
    let connection = McpClient::connect(config(&mock.url, None), CancellationToken::new())
        .await
        .unwrap();
    let tools = connection
        .client
        .list_tools(&CancellationToken::new())
        .await
        .unwrap();
    let slow = tools
        .tools
        .iter()
        .find(|tool| tool.name == "slow_progress")
        .unwrap();
    let progress: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = progress.clone();
    let result = connection
        .client
        .call_tool(
            "slow_progress",
            &json!({"seconds": 1}),
            Some(slow),
            None,
            Some(Arc::new(move |event| {
                sink.lock().unwrap().push(event);
            })),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(progress.lock().unwrap().len(), 3);
    assert_eq!(result.content[0]["text"], "slept 1s");

    let elicit = tools
        .tools
        .iter()
        .find(|tool| tool.name == "elicit")
        .unwrap();
    let first = connection
        .client
        .call_tool(
            "elicit",
            &json!({"question": "Name?"}),
            Some(elicit),
            None,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(first.result_type(), "input_required");
    assert!(first.input_requests.is_some());
    let second = connection
        .client
        .call_tool(
            "elicit",
            &json!({"question": "Name?"}),
            Some(elicit),
            Some(MrtrRetry {
                input_responses: json!({"answer": {"action": "accept", "content": {"name": "ari"}}}),
                request_state: first.request_state,
            }),
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(second.content[0]["text"], "answer: ari");
}

#[tokio::test]
async fn legacy_fallback_and_sse_transport() {
    let legacy = mcp::start(McpMode::Legacy).await;
    let connection = McpClient::connect(config(&legacy.url, None), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(connection.negotiated.protocol_version, "2025-11-25");
    assert_eq!(connection.negotiated.transport, TransportKind::LegacyHttp);
    let tools = connection
        .client
        .list_tools(&CancellationToken::new())
        .await
        .unwrap();
    assert!(tools.tools.iter().any(|tool| tool.name == "echo"));

    let sse = mcp::start(McpMode::Sse).await;
    let connection = McpClient::connect(config(&sse.url, None), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(connection.negotiated.transport, TransportKind::Sse);
    let tools = connection
        .client
        .list_tools(&CancellationToken::new())
        .await
        .unwrap();
    let echo = tools.tools.iter().find(|tool| tool.name == "echo").unwrap();
    let result = connection
        .client
        .call_tool(
            "echo",
            &json!({"text":"sse"}),
            Some(echo),
            None,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.content[0]["text"], "echo: sse");
}

#[tokio::test]
async fn header_mirroring_and_large_payload() {
    let mock = mcp::start(McpMode::Modern).await;
    let connection = McpClient::connect(config(&mock.url, None), CancellationToken::new())
        .await
        .unwrap();
    let tools = connection
        .client
        .list_tools(&CancellationToken::new())
        .await
        .unwrap();
    let header = tools
        .tools
        .iter()
        .find(|tool| tool.name == "header_echo")
        .unwrap();
    connection
        .client
        .call_tool(
            "header_echo",
            &json!({"region": "us-west1"}),
            Some(header),
            None,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = mock.requests();
    let call = requests
        .iter()
        .find(|request| request.method == "tools/call")
        .unwrap();
    assert_eq!(
        call.headers.get("mcp-param-region").map(String::as_str),
        Some("us-west1")
    );

    let big = tools.tools.iter().find(|tool| tool.name == "big").unwrap();
    let result = connection
        .client
        .call_tool(
            "big",
            &json!({"n": 50000}),
            Some(big),
            None,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.content[0]["text"].as_str().unwrap().len(), 50000);
}

#[tokio::test]
async fn json_rpc_error_is_humanized() {
    let mock = mcp::start(McpMode::Modern).await;
    let connection = McpClient::connect(config(&mock.url, None), CancellationToken::new())
        .await
        .unwrap();
    let tools = connection
        .client
        .list_tools(&CancellationToken::new())
        .await
        .unwrap();
    let fail = tools.tools.iter().find(|tool| tool.name == "fail").unwrap();
    let error = connection
        .client
        .call_tool(
            "fail",
            &json!({}),
            Some(fail),
            None,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.user_message().contains("mock failure"));
}

#[tokio::test]
async fn sse_hint_on_an_mcp_url_still_negotiates_http() {
    let mock = mcp::start(McpMode::Modern).await;
    let connection = McpClient::connect(
        config(&mock.url, Some(TransportKind::Sse)),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(connection.negotiated.protocol_version, "2026-07-28");
    assert_eq!(
        connection.negotiated.transport,
        TransportKind::StreamableHttp
    );
}
