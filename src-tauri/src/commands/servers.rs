use serde::Deserialize;
use serde_json::Value;
use tauri::State;
use tokio_util::sync::CancellationToken;

use mcp_chat_core::db::repo::{permissions, servers};
use mcp_chat_core::mcp::client::{
    args_from_json, env_from_json, headers_from_json, validate_server_url,
};
use mcp_chat_core::mcp::manager::ConnectionReport;
use mcp_chat_core::mcp::types::{CallToolResult, TransportKind};
use mcp_chat_core::models::{AuthKind, Server, ServerPolicy, ServerView};
use mcp_chat_core::secrets::{server_api_key_account, server_bearer_account};
use mcp_chat_core::util::slugify;

use crate::error::{CmdResult, cmd_err};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInput {
    pub name: String,
    pub url: String,
    pub auth_kind: String,
    pub headers_json: Option<String>,
    pub policy: String,
    pub bearer: Option<String>,
    pub api_key: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub args_json: Option<String>,
    pub env_json: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestServerInput {
    pub server_id: Option<String>,
    pub name: String,
    pub url: String,
    pub auth_kind: String,
    pub headers_json: Option<String>,
    pub bearer: Option<String>,
    pub api_key: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub args_json: Option<String>,
    pub env_json: Option<String>,
    pub cwd: Option<String>,
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

fn validate(input: &ServerInput) -> CmdResult<ValidServer> {
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
        args_from_json(&args_json).map_err(cmd_err)?;
        let env_json = input.env_json.clone().unwrap_or_else(|| "{}".into());
        env_from_json(&env_json).map_err(cmd_err)?;
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

    validate_server_url(&input.url).map_err(cmd_err)?;
    if AuthKind::parse(&input.auth_kind).is_none() {
        return Err("Unknown auth kind".into());
    }
    let headers_json = input.headers_json.clone().unwrap_or_else(|| "{}".into());
    headers_from_json(&headers_json).map_err(cmd_err)?;
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
        tool_count,
    }
}

#[tauri::command]
pub async fn list_servers(state: State<'_, AppState>) -> CmdResult<Vec<ServerView>> {
    let list = servers::list(&state.db.pool).await.map_err(cmd_err)?;
    let mut out = Vec::new();
    for server in list {
        out.push(view(state.inner(), server).await);
    }
    Ok(out)
}

#[tauri::command]
pub async fn create_server(
    state: State<'_, AppState>,
    input: ServerInput,
) -> CmdResult<ServerView> {
    let valid = validate(&input)?;
    if valid.auth_kind == "oauth" {
        return Err(
            "OAuth servers are set up in the OAuth milestone; use a bearer token or none for now."
                .into(),
        );
    }
    let slug = unique_slug(&state, &input.name).await;
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
    .map_err(cmd_err)?;
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
                .map_err(cmd_err)?;
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
                .map_err(cmd_err)?;
        }
    }
    let _ = state.mcp.refresh_tools(&server.id).await;
    let server = servers::get(&state.db.pool, &server.id)
        .await
        .map_err(cmd_err)?;
    Ok(view(state.inner(), server).await)
}

#[tauri::command]
pub async fn update_server(
    state: State<'_, AppState>,
    server_id: String,
    input: ServerInput,
) -> CmdResult<ServerView> {
    let valid = validate(&input)?;
    if valid.auth_kind == "oauth" {
        return Err("OAuth servers are set up in the OAuth milestone.".into());
    }
    let existing = servers::get(&state.db.pool, &server_id)
        .await
        .map_err(cmd_err)?;
    servers::update(
        &state.db.pool,
        &server_id,
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
    .map_err(cmd_err)?;
    if valid.auth_kind == "bearer" {
        if let Some(token) = input
            .bearer
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        {
            state
                .secrets
                .set(&server_bearer_account(&server_id), token.trim())
                .await
                .map_err(cmd_err)?;
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
                .set(&server_api_key_account(&server_id), token.trim())
                .await
                .map_err(cmd_err)?;
        }
    }
    if valid.auth_kind != "bearer" {
        let _ = state
            .secrets
            .delete(&server_bearer_account(&server_id))
            .await;
    }
    if valid.auth_kind != "api_key" {
        let _ = state
            .secrets
            .delete(&server_api_key_account(&server_id))
            .await;
    }
    state.mcp.invalidate(&server_id).await;
    let _ = state.mcp.refresh_tools(&server_id).await;
    let update = servers::get(&state.db.pool, &server_id)
        .await
        .map_err(cmd_err)?;
    Ok(view(&state, update).await)
}

#[tauri::command]
pub async fn set_server_enabled(
    state: State<'_, AppState>,
    server_id: String,
    enabled: bool,
) -> CmdResult<()> {
    servers::set_enabled(&state.db.pool, &server_id, enabled)
        .await
        .map_err(cmd_err)?;
    if !enabled {
        state.mcp.invalidate(&server_id).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn set_server_bearer(
    state: State<'_, AppState>,
    server_id: String,
    token: String,
) -> CmdResult<()> {
    let server = servers::get(&state.db.pool, &server_id)
        .await
        .map_err(cmd_err)?;
    if server.auth_kind != "bearer" {
        return Err("This server is not configured for a bearer token.".into());
    }
    state
        .secrets
        .set(&server_bearer_account(&server_id), token.trim())
        .await
        .map_err(cmd_err)?;
    state.mcp.invalidate(&server_id).await;
    Ok(())
}

#[tauri::command]
pub async fn delete_server(state: State<'_, AppState>, server_id: String) -> CmdResult<()> {
    state
        .secrets
        .delete(&server_bearer_account(&server_id))
        .await
        .ok();
    state
        .secrets
        .delete(&server_api_key_account(&server_id))
        .await
        .ok();
    state.mcp.invalidate(&server_id).await;
    servers::delete(&state.db.pool, &server_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn test_server(
    state: State<'_, AppState>,
    input: TestServerInput,
) -> CmdResult<ConnectionReport> {
    let transport = input.transport.as_deref().and_then(TransportKind::parse);
    if transport == Some(TransportKind::Stdio) {
        let command = input.command.clone().unwrap_or_default();
        if command.trim().is_empty() {
            return Err("A command is required for a local MCP server".into());
        }
        let args_json = input.args_json.clone().unwrap_or_else(|| "[]".into());
        let args = args_from_json(&args_json).map_err(cmd_err)?;
        let env_json = input.env_json.clone().unwrap_or_else(|| "{}".into());
        let env = env_from_json(&env_json).map_err(cmd_err)?;
        let cwd = input
            .cwd
            .clone()
            .map(|cwd| cwd.trim().to_owned())
            .filter(|cwd| !cwd.is_empty());
        return state
            .mcp
            .test_stdio(
                &input.name,
                &slugify(&input.name),
                command.trim(),
                args,
                env,
                cwd,
            )
            .await
            .map_err(cmd_err);
    }
    validate_server_url(&input.url).map_err(cmd_err)?;
    let headers_json = input.headers_json.clone().unwrap_or_else(|| "{}".into());
    let mut headers = headers_from_json(&headers_json).map_err(cmd_err)?;
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
        return Err("OAuth servers are set up in the OAuth milestone.".into());
    }
    state
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
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn refresh_server(
    state: State<'_, AppState>,
    server_id: String,
) -> CmdResult<ConnectionReport> {
    state.mcp.report(&server_id).await.map_err(cmd_err)
}

#[tauri::command]
pub async fn refresh_all_servers(state: State<'_, AppState>) -> CmdResult<()> {
    state.mcp.refresh_all().await;
    Ok(())
}

#[tauri::command]
pub async fn list_server_tools(
    state: State<'_, AppState>,
    server_id: String,
) -> CmdResult<Vec<mcp_chat_core::mcp::types::ToolInfo>> {
    state.mcp.refresh_tools(&server_id).await.map_err(cmd_err)
}

#[tauri::command]
pub async fn run_server_tool(
    state: State<'_, AppState>,
    server_id: String,
    tool_name: String,
    arguments: Value,
) -> CmdResult<CallToolResult> {
    let server = servers::get(&state.db.pool, &server_id)
        .await
        .map_err(cmd_err)?;
    let tools = state.mcp.refresh_tools(&server_id).await.map_err(cmd_err)?;
    let tool = tools
        .iter()
        .find(|tool| tool.name == tool_name)
        .cloned()
        .ok_or_else(|| format!("No tool named {tool_name:?} on this server"))?;
    state
        .mcp
        .call_tool(
            &server.id,
            &tool_name,
            &arguments,
            Some(&tool),
            None,
            None,
            &CancellationToken::new(),
        )
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn list_permission_rules(
    state: State<'_, AppState>,
    server_id: Option<String>,
) -> CmdResult<Vec<mcp_chat_core::models::PermissionRule>> {
    match server_id {
        Some(id) => permissions::list_for_server(&state.db.pool, &id)
            .await
            .map_err(cmd_err),
        None => permissions::list(&state.db.pool).await.map_err(cmd_err),
    }
}

#[tauri::command]
pub async fn delete_permission_rule(state: State<'_, AppState>, rule_id: String) -> CmdResult<()> {
    permissions::delete(&state.db.pool, &rule_id)
        .await
        .map_err(cmd_err)
}
