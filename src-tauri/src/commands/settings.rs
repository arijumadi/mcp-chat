use std::collections::HashMap;

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use url::Url;

use mcp_chat_core::db::repo::{conversations, permissions, providers, servers, settings};
use mcp_chat_core::secrets::{oauth_account, provider_account, server_bearer_account};

use crate::error::{CmdResult, cmd_err};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub data_dir: String,
    pub platform: String,
}

#[tauri::command]
pub async fn app_info(state: State<'_, AppState>) -> CmdResult<AppInfo> {
    Ok(AppInfo {
        name: "MCP Chat".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        data_dir: state.data_dir.display().to_string(),
        platform: std::env::consts::OS.into(),
    })
}

#[tauri::command]
pub async fn list_settings(state: State<'_, AppState>) -> CmdResult<HashMap<String, String>> {
    let values = settings::all(&state.db.pool).await.map_err(cmd_err)?;
    Ok(values.into_iter().collect())
}

#[tauri::command]
pub async fn set_setting(state: State<'_, AppState>, key: String, value: String) -> CmdResult<()> {
    if key.trim().is_empty() {
        return Err("Setting key is empty".into());
    }
    settings::set(&state.db.pool, key.trim(), &value)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn delete_setting(state: State<'_, AppState>, key: String) -> CmdResult<()> {
    settings::delete(&state.db.pool, &key)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn delete_all_data(state: State<'_, AppState>) -> CmdResult<()> {
    let provider_list = providers::list(&state.db.pool).await.map_err(cmd_err)?;
    for provider in &provider_list {
        let account = provider
            .api_key_ref
            .clone()
            .unwrap_or_else(|| provider.id.clone());
        let _ = state.secrets.delete(&provider_account(&account)).await;
        let _ = state.secrets.delete(&provider.id).await;
    }

    let server_list = servers::list(&state.db.pool).await.map_err(cmd_err)?;
    for server in &server_list {
        let _ = state
            .secrets
            .delete(&server_bearer_account(&server.id))
            .await;
        if let Some(issuer) = &server.oauth_issuer {
            for suffix in ["access_token", "refresh_token", "client_secret"] {
                let _ = state
                    .secrets
                    .delete(&oauth_account(&server.id, issuer, suffix))
                    .await;
            }
        }
        state.mcp.invalidate(&server.id).await;
    }

    let open_conversations = conversations::list_all(&state.db.pool)
        .await
        .map_err(cmd_err)?;
    for conversation in &open_conversations {
        state.engine.stop(&conversation.id);
    }
    conversations::delete_all(&state.db.pool)
        .await
        .map_err(cmd_err)?;
    providers::delete_all(&state.db.pool)
        .await
        .map_err(cmd_err)?;
    servers::delete_all(&state.db.pool).await.map_err(cmd_err)?;
    permissions::delete_all(&state.db.pool)
        .await
        .map_err(cmd_err)?;
    settings::delete_all(&state.db.pool)
        .await
        .map_err(cmd_err)?;

    let _ = tokio::fs::remove_dir_all(&state.output_dir).await;
    let _ = tokio::fs::create_dir_all(&state.output_dir).await;
    let _ = tokio::fs::remove_dir_all(state.data_dir.join("logs")).await;
    Ok(())
}

#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> CmdResult<()> {
    let parsed = Url::parse(url.trim()).map_err(|_| "Invalid URL".to_string())?;
    if !matches!(parsed.scheme(), "https" | "http" | "mailto") {
        return Err("Only https, http and mailto links can be opened".into());
    }
    app.opener()
        .open_url(parsed.as_str(), None::<&str>)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn open_data_dir(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    app.opener()
        .open_path(state.data_dir.display().to_string(), None::<&str>)
        .map_err(|err| err.to_string())
}
