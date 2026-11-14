use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use url::Url;

use mcp_chat_core::db::Db;
use mcp_chat_core::db::repo::servers;
use mcp_chat_core::mcp::McpManager;
use mcp_chat_core::secrets::MemoryStore;

#[derive(Clone)]
struct MockState {
    base: String,
}

async fn mcp(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "Bearer access-test" || value == "Bearer access-refreshed");
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                format!(
                    "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
                    state.base
                ),
            )],
        )
            .into_response();
    }
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = match method {
        "server/discover" => json!({
            "supportedVersions": ["2026-07-28"],
            "_meta": {
                "io.modelcontextprotocol/serverInfo": {"name": "mock-oauth", "version": "1.0.0"}
            },
            "capabilities": {"tools": {}},
            "instructions": "mock OAuth server"
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "echo",
                "description": "Echo a value",
                "inputSchema": {"type": "object", "properties": {"value": {"type": "string"}}}
            }]
        }),
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!("unexpected MCP method {other}"),
            )
                .into_response();
        }
    };
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

async fn protected_resource(State(state): State<MockState>) -> Json<Value> {
    Json(json!({
        "resource": format!("{}/mcp", state.base),
        "authorization_servers": [state.base],
        "scopes_supported": ["mcp.read", "mcp.write"]
    }))
}

async fn authorization_server(State(state): State<MockState>) -> Json<Value> {
    Json(json!({
        "issuer": state.base,
        "authorization_endpoint": format!("{}/authorize", state.base),
        "token_endpoint": format!("{}/token", state.base),
        "registration_endpoint": format!("{}/register", state.base),
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": ["mcp.read", "mcp.write"]
    }))
}

async fn register() -> Json<Value> {
    Json(json!({
        "client_id": "client-test",
        "token_endpoint_auth_method": "none"
    }))
}

async fn token(Form(form): Form<HashMap<String, String>>) -> Json<Value> {
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            assert_eq!(form.get("code").map(String::as_str), Some("test-code"));
            assert_eq!(
                form.get("client_id").map(String::as_str),
                Some("client-test")
            );
            assert!(form.contains_key("code_verifier"));
            Json(json!({
                "access_token": "access-test",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "refresh-test",
                "scope": "mcp.read mcp.write"
            }))
        }
        Some("refresh_token") => {
            assert_eq!(
                form.get("refresh_token").map(String::as_str),
                Some("refresh-test")
            );
            assert_eq!(
                form.get("client_id").map(String::as_str),
                Some("client-test")
            );
            Json(json!({
                "access_token": "access-refreshed",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "refresh-test-2",
                "scope": "mcp.read mcp.write"
            }))
        }
        other => panic!("unexpected grant type: {other:?}"),
    }
}

async fn start_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let base = format!("http://{address}");
    let state = MockState { base: base.clone() };
    let app = Router::new()
        .route("/mcp", post(mcp))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .route("/register", post(register))
        .route("/token", post(token))
        .with_state(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, handle)
}

#[tokio::test]
async fn oauth_discovery_registration_and_callback_store_a_session() {
    let (base, mock) = start_mock().await;
    let db = Db::open_memory().await.unwrap();
    let secrets = Arc::new(MemoryStore::default());
    let manager = Arc::new(McpManager::new(db.clone(), secrets.clone()));
    let server = servers::create(
        &db.pool,
        &servers::ServerConfig::http(
            "T3 Code",
            "t3_code",
            format!("{base}/mcp"),
            "streamable_http",
            "oauth",
            "{}",
            "ask",
        ),
    )
    .await
    .unwrap();

    let start = manager.start_oauth(&server).await.unwrap();
    let auth = Url::parse(&start.auth_url).unwrap();
    let params: HashMap<_, _> = auth.query_pairs().into_owned().collect();
    assert_eq!(
        params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert_eq!(
        params.get("client_id").map(String::as_str),
        Some("client-test")
    );
    let expected_resource = format!("{base}/mcp");
    assert_eq!(
        params.get("resource").map(String::as_str),
        Some(expected_resource.as_str())
    );
    let state = params.get("state").cloned().unwrap();
    let redirect = start.redirect_uri.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let callback = format!("{redirect}?code=test-code&state={state}");
        let response = reqwest::get(callback).await.unwrap();
        assert!(response.status().is_success());
    });

    let mut status = manager.oauth_status(&start.flow_id).await;
    for _ in 0..50 {
        if status.status != "pending" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        status = manager.oauth_status(&start.flow_id).await;
    }
    assert_eq!(status.status, "success", "{:?}", status.error);
    assert!(manager.has_oauth_token(&server).await);
    let saved = manager.oauth_token(&server).await.unwrap().unwrap();
    assert_eq!(saved.access_token, "access-test");
    assert_eq!(saved.refresh_token.as_deref(), Some("refresh-test"));

    let tools = manager.refresh_tools(&server.id).await.unwrap();
    assert!(tools.iter().any(|tool| tool.name == "echo"));

    let refreshed = manager.force_refresh_oauth(&server).await.unwrap();
    assert_eq!(refreshed.access_token, "access-refreshed");
    assert_eq!(refreshed.refresh_token.as_deref(), Some("refresh-test-2"));
    let stored = servers::get(&db.pool, &server.id).await.unwrap();
    assert_eq!(stored.oauth_issuer.as_deref(), Some(base.as_str()));
    assert_eq!(stored.oauth_client_id.as_deref(), Some("client-test"));
    mock.abort();
}
