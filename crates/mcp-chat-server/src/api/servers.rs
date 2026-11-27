//! MCP servers.
//!
//! Ported from `src-tauri/src/commands/servers.rs`. OAuth is included but will
//! only complete when the browser can reach this host's callback port; the
//! server prints a warning when the flow starts.

use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use mcp_chat_core::db::repo::{permissions, servers};
use mcp_chat_core::mcp::client::{
    args_from_json, env_from_json, headers_from_json, validate_server_url,
};
use mcp_chat_core::mcp::manager::ConnectionReport;
use mcp_chat_core::mcp::oauth::{OAuthStart, OAuthStatus};
use mcp_chat_core::mcp::types::{CallToolResult, TransportKind};
use mcp_chat_core::models::{AuthKind, Server, ServerPolicy, ServerView};
use mcp_chat_core::secrets::{server_api_key_account, server_bearer_account};
use mcp_chat_core::util::slugify;

use super::{err, parse, to_value};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerInput {
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    auth_kind: String,
    #[serde(default)]
    headers_json: Option<String>,
    #[serde(default)]
    policy: String,
    #[serde(default)]
    bearer: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args_json: Option<String>,
    #[serde(default)]
    env_json: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestServerInput {
    #[serde(default)]
    server_id: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    auth_kind: String,
    #[serde(default)]
    headers_json: Option<String>,
    #[serde(default)]
    bearer: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args_json: Option<String>,
    #[serde(default)]
    env_json: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

struct ValidServer {
    name: String,
    url: String,
    transport: TransportKind,
    auth_kind: String,
    headers_json: String,
    policy: String,
    command: String,
    args_json: String,
    env_json: String,
    cwd: String,
}

fn validate(input: &ServerInput) -> Result<ValidServer, String> {
    if input.name.trim().is_empty() {
        return Err("Server name is required".into());
    }
    if ServerPolicy::parse(&input.policy).is_none() {
        return Err("Unknown policy".into());
    }
    let transport = match input.transport.as_deref() {
        Some(value) => {
            TransportKind::parse(value).ok_or_else(|| "Unknown transport".to_string())?
        }
        None => TransportKind::StreamableHttp,
    };
    if transport == TransportKind::Stdio {
        let command = input.command.clone().unwrap_or_default();
        if command.trim().is_empty() {
            return Err("A command is required for a local MCP server".into());
        }
        let args_json = input.args_json.clone().unwrap_or_else(|| "[]".into());
        args_from_json(&args_json).map_err(err)?;
        let env_json = input.env_json.clone().unwrap_or_else(|| "{}".into());
        env_from_json(&env_json).map_err(err)?;
        return Ok(ValidServer {
            name: input.name.trim().to_owned(),
            url: String::new(),
            transport,
            auth_kind: "none".into(),
            headers_json: "{}".into(),
            policy: input.policy.clone(),
            command: command.trim().to_owned(),
            args_json,
            env_json,
            cwd: input.cwd.clone().unwrap_or_default().trim().to_owned(),
        });
    }

    validate_server_url(&input.url).map_err(err)?;
    if AuthKind::parse(&input.auth_kind).is_none() {
        return Err("Unknown auth kind".into());
    }
    let headers_json = input.headers_json.clone().unwrap_or_else(|| "{}".into());
    headers_from_json(&headers_json).map_err(err)?;
    Ok(ValidServer {
        name: input.name.trim().to_owned(),
        url: input.url.trim().to_owned(),
        transport,
        auth_kind: input.auth_kind.clone(),
        headers_json,
        policy: input.policy.clone(),
        command: String::new(),
        args_json: "[]".into(),
        env_json: "{}".into(),
        cwd: String::new(),
    })
}

async fn unique_slug(state: &AppState, name: &str) -> String {
    let base = slugify(name);
    let mut candidate = base.clone();
    let mut suffix = 2;
    while servers::get_by_slug(&state.db.pool, &candidate)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        candidate = format!("{base}_{suffix}");
        suffix += 1;
    }
    candidate
}

async fn view(state: &AppState, server: Server) -> ServerView {
    let has_bearer = if server.auth_kind == "bearer" {
        state
            .secrets
            .get(&server_bearer_account(&server.id))
            .await
            .ok()
            .flatten()
            .is_some()
    } else {
        false
    };
    let has_api_key = if server.auth_kind == "api_key" {
        state
            .secrets
            .get(&server_api_key_account(&server.id))
            .await
            .ok()
            .flatten()
            .is_some()
    } else {
        false
    };
    let has_oauth_token = if server.auth_kind == "oauth" {
        state.mcp.has_oauth_token(&server).await
    } else {
        false
    };
    let tool_count = server
        .tools_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<Value>>(json).ok())
        .map(|tools| tools.len())
        .unwrap_or(0);
    ServerView {
        server,
        has_bearer,
        has_api_key,
        has_oauth_token,
        tool_count,
    }
}

pub async fn list_servers(state: &AppState, _args: Value) -> Result<Value, String> {
    let list = servers::list(&state.db.pool).await.map_err(err)?;
    let mut out = Vec::new();
    for server in list {
        out.push(view(state, server).await);
    }
    to_value(out)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateServer {
    input: ServerInput,
}

pub async fn create_server(state: &AppState, args: Value) -> Result<Value, String> {
    let params: CreateServer = parse(args)?;
    let input = params.input;
    let valid = validate(&input)?;
    let slug = unique_slug(state, &input.name).await;
    let server = servers::create(
        &state.db.pool,
        &servers::ServerConfig {
            name: valid.name,
            slug,
            url: valid.url,
            transport: valid.transport.as_str().into(),
            command: valid.command,
            args_json: valid.args_json,
            env_json: valid.env_json,
            cwd: valid.cwd,
            auth_kind: valid.auth_kind.clone(),
            headers_json: valid.headers_json,
            policy: valid.policy,
        },
    )
    .await
    .map_err(err)?;
    if valid.auth_kind == "bearer" {
        if let Some(token) = input
            .bearer
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        {
            state
                .secrets
                .set(&server_bearer_account(&server.id), token.trim())
                .await
                .map_err(err)?;
        }
    }
    if valid.auth_kind == "api_key" {
        if let Some(token) = input
            .api_key
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        {
            state
                .secrets
                .set(&server_api_key_account(&server.id), token.trim())
                .await
                .map_err(err)?;
        }
    }
    if valid.auth_kind != "oauth" {
        let _ = state.mcp.refresh_tools(&server.id).await;
    }
    let server = servers::get(&state.db.pool, &server.id)
        .await
        .map_err(err)?;
    to_value(view(state, server).await)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateServer {
    server_id: String,
    input: ServerInput,
}

pub async fn update_server(state: &AppState, args: Value) -> Result<Value, String> {
    let params: UpdateServer = parse(args)?;
    let input = params.input;
    let valid = validate(&input)?;
    let existing = servers::get(&state.db.pool, &params.server_id)
        .await
        .map_err(err)?;
    let new_url = valid.url.clone();
    servers::update(
        &state.db.pool,
        &params.server_id,
        &servers::ServerConfig {
            name: valid.name,
            slug: existing.slug.clone(),
            url: valid.url,
            transport: valid.transport.as_str().into(),
            command: valid.command,
            args_json: valid.args_json,
            env_json: valid.env_json,
            cwd: valid.cwd,
            auth_kind: valid.auth_kind.clone(),
            headers_json: valid.headers_json,
            policy: valid.policy,
        },
        existing.enabled,
    )
    .await
    .map_err(err)?;
    if valid.auth_kind == "bearer" {
        if let Some(token) = input
            .bearer
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        {
            state
                .secrets
                .set(&server_bearer_account(&params.server_id), token.trim())
                .await
                .map_err(err)?;
        }
    }
    if valid.auth_kind == "api_key" {
        if let Some(token) = input
            .api_key
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        {
            state
                .secrets
                .set(&server_api_key_account(&params.server_id), token.trim())
                .await
                .map_err(err)?;
        }
    }
    if valid.auth_kind != "bearer" {
        let _ = state
            .secrets
            .delete(&server_bearer_account(&params.server_id))
            .await;
    }
    if valid.auth_kind != "api_key" {
        let _ = state
            .secrets
            .delete(&server_api_key_account(&params.server_id))
            .await;
    }
    let reset_oauth =
        existing.url != new_url || (existing.auth_kind == "oauth") != (valid.auth_kind == "oauth");
    if reset_oauth {
        state.mcp.disconnect_oauth(&existing).await.map_err(err)?;
    }
    state.mcp.invalidate(&params.server_id).await;
    let update = servers::get(&state.db.pool, &params.server_id)
        .await
        .map_err(err)?;
    if valid.auth_kind != "oauth" || state.mcp.has_oauth_token(&update).await {
        let _ = state.mcp.refresh_tools(&params.server_id).await;
    }
    to_value(view(state, update).await)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerId {
    server_id: String,
}

pub async fn delete_server(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ServerId = parse(args)?;
    if let Ok(server) = servers::get(&state.db.pool, &params.server_id).await {
        let _ = state.mcp.disconnect_oauth(&server).await;
    }
    state
        .secrets
        .delete(&server_bearer_account(&params.server_id))
        .await
        .ok();
    state
        .secrets
        .delete(&server_api_key_account(&params.server_id))
        .await
        .ok();
    state.mcp.invalidate(&params.server_id).await;
    servers::delete(&state.db.pool, &params.server_id)
        .await
        .map_err(err)?;
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetServerEnabled {
    server_id: String,
    enabled: bool,
}

pub async fn set_server_enabled(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetServerEnabled = parse(args)?;
    servers::set_enabled(&state.db.pool, &params.server_id, params.enabled)
        .await
        .map_err(err)?;
    if !params.enabled {
        state.mcp.invalidate(&params.server_id).await;
    }
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetServerBearer {
    server_id: String,
    token: String,
}

pub async fn set_server_bearer(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetServerBearer = parse(args)?;
    let server = servers::get(&state.db.pool, &params.server_id)
        .await
        .map_err(err)?;
    if server.auth_kind != "bearer" {
        return Err("This server is not configured for a bearer token.".into());
    }
    state
        .secrets
        .set(
            &server_bearer_account(&params.server_id),
            params.token.trim(),
        )
        .await
        .map_err(err)?;
    state.mcp.invalidate(&params.server_id).await;
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestServer {
    input: TestServerInput,
}

pub async fn test_server(state: &AppState, args: Value) -> Result<Value, String> {
    let params: TestServer = parse(args)?;
    let input = params.input;
    let transport = input.transport.as_deref().and_then(TransportKind::parse);
    if transport == Some(TransportKind::Stdio) {
        let command = input.command.clone().unwrap_or_default();
        if command.trim().is_empty() {
            return Err("A command is required for a local MCP server".into());
        }
        let args_json = input.args_json.clone().unwrap_or_else(|| "[]".into());
        let parsed_args = args_from_json(&args_json).map_err(err)?;
        let env_json = input.env_json.clone().unwrap_or_else(|| "{}".into());
        let parsed_env = env_from_json(&env_json).map_err(err)?;
        let cwd = input
            .cwd
            .clone()
            .map(|cwd| cwd.trim().to_owned())
            .filter(|cwd| !cwd.is_empty());
        let report: ConnectionReport = state
            .mcp
            .test_stdio(
                &input.name,
                &slugify(&input.name),
                command.trim(),
                parsed_args,
                parsed_env,
                cwd,
            )
            .await
            .map_err(err)?;
        return to_value(report);
    }
    validate_server_url(&input.url).map_err(err)?;
    let headers_json = input.headers_json.clone().unwrap_or_else(|| "{}".into());
    let mut headers = headers_from_json(&headers_json).map_err(err)?;
    if input.auth_kind == "api_key" {
        let mut api_key = input
            .api_key
            .clone()
            .filter(|token| !token.trim().is_empty());
        if api_key.is_none() {
            if let Some(server_id) = input.server_id.as_deref() {
                api_key = state
                    .secrets
                    .get(&server_api_key_account(server_id))
                    .await
                    .ok()
                    .flatten();
            }
        }
        if let Some(token) = api_key {
            headers.insert("X-API-Key".into(), token);
        }
    }
    let mut bearer = input
        .bearer
        .clone()
        .filter(|token| !token.trim().is_empty());
    if bearer.is_none() && input.auth_kind == "bearer" {
        if let Some(server_id) = input.server_id.as_deref() {
            bearer = state
                .secrets
                .get(&server_bearer_account(server_id))
                .await
                .ok()
                .flatten();
        }
    }
    if input.auth_kind == "oauth" {
        let server_id = input
            .server_id
            .as_deref()
            .ok_or_else(|| "Save this server and connect OAuth before testing it.".to_string())?;
        let server = servers::get(&state.db.pool, server_id).await.map_err(err)?;
        let report = state.mcp.test_saved(&server).await.map_err(err)?;
        return to_value(report);
    }
    let report = state
        .mcp
        .test(
            &input.name,
            &slugify(&input.name),
            &input.url,
            headers,
            bearer,
            transport,
        )
        .await
        .map_err(err)?;
    to_value(report)
}

pub async fn refresh_server(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ServerId = parse(args)?;
    let report = state.mcp.report(&params.server_id).await.map_err(err)?;
    to_value(report)
}

pub async fn refresh_all_servers(state: &AppState, _args: Value) -> Result<Value, String> {
    state.mcp.refresh_all().await;
    to_value(())
}

pub async fn list_server_tools(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ServerId = parse(args)?;
    let tools = state
        .mcp
        .refresh_tools(&params.server_id)
        .await
        .map_err(err)?;
    to_value(tools)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunServerTool {
    server_id: String,
    tool_name: String,
    #[serde(default)]
    arguments: Value,
}

pub async fn run_server_tool(state: &AppState, args: Value) -> Result<Value, String> {
    let params: RunServerTool = parse(args)?;
    let server = servers::get(&state.db.pool, &params.server_id)
        .await
        .map_err(err)?;
    let tools = state
        .mcp
        .refresh_tools(&params.server_id)
        .await
        .map_err(err)?;
    let tool = tools
        .iter()
        .find(|tool| tool.name == params.tool_name)
        .cloned()
        .ok_or_else(|| format!("No tool named `{}` on this server", params.tool_name))?;
    let result: CallToolResult = state
        .mcp
        .call_tool(
            &server.id,
            &params.tool_name,
            &params.arguments,
            Some(&tool),
            None,
            None,
            &CancellationToken::new(),
        )
        .await
        .map_err(err)?;
    to_value(result)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListPermissionRules {
    #[serde(default)]
    server_id: Option<String>,
}

pub async fn list_permission_rules(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ListPermissionRules = parse(args)?;
    let rules = match params.server_id {
        Some(id) => permissions::list_for_server(&state.db.pool, &id)
            .await
            .map_err(err)?,
        None => permissions::list(&state.db.pool).await.map_err(err)?,
    };
    to_value(rules)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeletePermissionRule {
    rule_id: String,
}

pub async fn delete_permission_rule(state: &AppState, args: Value) -> Result<Value, String> {
    let params: DeletePermissionRule = parse(args)?;
    permissions::delete(&state.db.pool, &params.rule_id)
        .await
        .map_err(err)?;
    to_value(())
}

pub async fn start_server_oauth(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ServerId = parse(args)?;
    let server = servers::get(&state.db.pool, &params.server_id)
        .await
        .map_err(err)?;
    if server.auth_kind != "oauth" {
        return Err("Set this server's auth mode to OAuth first.".into());
    }
    let start: OAuthStart = state.mcp.start_oauth(&server).await.map_err(err)?;
    tracing::warn!(
        "OAuth started. The callback listens on this host's loopback; finish the flow on this \
         machine or forward the port, otherwise the redirect will not arrive."
    );
    to_value(start)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuthStatusQuery {
    flow_id: String,
}

pub async fn oauth_status(state: &AppState, args: Value) -> Result<Value, String> {
    let params: OAuthStatusQuery = parse(args)?;
    let status: OAuthStatus = state.mcp.oauth_status(&params.flow_id).await;
    to_value(status)
}

pub async fn cancel_server_oauth(state: &AppState, args: Value) -> Result<Value, String> {
    let params: OAuthStatusQuery = parse(args)?;
    state.mcp.cancel_oauth(&params.flow_id).await;
    to_value(())
}

pub async fn disconnect_server_oauth(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ServerId = parse(args)?;
    let server = servers::get(&state.db.pool, &params.server_id)
        .await
        .map_err(err)?;
    state.mcp.disconnect_oauth(&server).await.map_err(err)?;
    let server = servers::get(&state.db.pool, &params.server_id)
        .await
        .map_err(err)?;
    to_value(view(state, server).await)
}
