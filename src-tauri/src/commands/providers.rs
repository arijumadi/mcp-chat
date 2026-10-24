use serde::{Deserialize, Serialize};
use tauri::State;
use url::Url;

use mcp_chat_core::db::repo::{providers, settings};
use mcp_chat_core::llm;
use mcp_chat_core::models::{ProviderKind, ProviderView};
use mcp_chat_core::secrets::provider_account;
use mcp_chat_core::util::redact_secret;

use crate::error::{CmdResult, cmd_err};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub model: String,
    pub context_window: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub api_key: Option<String>,
    pub models: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyTest {
    pub ok: bool,
    pub models: Vec<String>,
    pub message: String,
}

fn validate(input: &ProviderInput) -> Result<(), String> {
    if input.name.trim().is_empty() {
        return Err("Provider name is required".into());
    }
    if ProviderKind::parse(&input.kind).is_none() {
        return Err("Unknown provider kind".into());
    }
    let url = Url::parse(input.base_url.trim()).map_err(|_| "Invalid base URL")?;
    match url.scheme() {
        "https" => {}
        "http" => {
            let host = url.host_str().unwrap_or("");
            if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
                return Err("HTTP is only allowed for localhost providers".into());
            }
        }
        _ => return Err("Base URL must use https://".into()),
    }
    Ok(())
}

async fn view(state: &AppState, provider: mcp_chat_core::models::Provider) -> ProviderView {
    let account = provider
        .api_key_ref
        .clone()
        .unwrap_or_else(|| provider.id.clone());
    let secret = state.secrets.get(&account).await.ok().flatten();
    let models = serde_json::from_str(&provider.models_json).unwrap_or_default();
    ProviderView {
        has_key: secret.is_some(),
        key_hint: secret.as_deref().map(redact_secret),
        models,
        provider,
    }
}

#[tauri::command]
pub async fn list_providers(state: State<'_, AppState>) -> CmdResult<Vec<ProviderView>> {
    let list = providers::list(&state.db.pool).await.map_err(cmd_err)?;
    let mut out = Vec::new();
    for provider in list {
        out.push(view(state.inner(), provider).await);
    }
    Ok(out)
}

#[tauri::command]
pub async fn create_provider(
    state: State<'_, AppState>,
    input: ProviderInput,
) -> CmdResult<ProviderView> {
    validate(&input)?;
    let provider = providers::create(
        &state.db.pool,
        input.name.trim(),
        &input.kind,
        input.base_url.trim().trim_end_matches('/'),
        input.model.trim(),
        None,
        input.context_window.unwrap_or(128_000),
        input.max_output_tokens.unwrap_or(8_192),
    )
    .await
    .map_err(cmd_err)?;
    if let Some(key) = input
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
    {
        state
            .secrets
            .set(&provider_account(&provider.id), key.trim())
            .await
            .map_err(cmd_err)?;
        providers::set_api_key_ref(&state.db.pool, &provider.id, Some(&provider.id))
            .await
            .map_err(cmd_err)?;
    }
    if let Some(models) = &input.models {
        providers::set_models(&state.db.pool, &provider.id, models)
            .await
            .map_err(cmd_err)?;
    }
    let provider = providers::get(&state.db.pool, &provider.id)
        .await
        .map_err(cmd_err)?;
    Ok(view(state.inner(), provider).await)
}

#[tauri::command]
pub async fn update_provider(
    state: State<'_, AppState>,
    provider_id: String,
    input: ProviderInput,
) -> CmdResult<ProviderView> {
    validate(&input)?;
    let provider = providers::update(
        &state.db.pool,
        &provider_id,
        input.name.trim(),
        &input.kind,
        input.base_url.trim().trim_end_matches('/'),
        input.model.trim(),
        input.context_window.unwrap_or(128_000),
        input.max_output_tokens.unwrap_or(8_192),
    )
    .await
    .map_err(cmd_err)?;
    if let Some(key) = input
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
    {
        state
            .secrets
            .set(&provider_account(&provider.id), key.trim())
            .await
            .map_err(cmd_err)?;
        providers::set_api_key_ref(&state.db.pool, &provider.id, Some(&provider.id))
            .await
            .map_err(cmd_err)?;
    }
    if let Some(models) = &input.models {
        providers::set_models(&state.db.pool, &provider.id, models)
            .await
            .map_err(cmd_err)?;
    }
    let provider = providers::get(&state.db.pool, &provider_id)
        .await
        .map_err(cmd_err)?;
    Ok(view(state.inner(), provider).await)
}

#[tauri::command]
pub async fn set_provider_key(
    state: State<'_, AppState>,
    provider_id: String,
    api_key: String,
) -> CmdResult<ProviderView> {
    if api_key.trim().is_empty() {
        return Err("API key is empty".into());
    }
    state
        .secrets
        .set(&provider_account(&provider_id), api_key.trim())
        .await
        .map_err(cmd_err)?;
    providers::set_api_key_ref(&state.db.pool, &provider_id, Some(&provider_id))
        .await
        .map_err(cmd_err)?;
    let provider = providers::get(&state.db.pool, &provider_id)
        .await
        .map_err(cmd_err)?;
    Ok(view(state.inner(), provider).await)
}

#[tauri::command]
pub async fn clear_provider_key(state: State<'_, AppState>, provider_id: String) -> CmdResult<()> {
    state
        .secrets
        .delete(&provider_account(&provider_id))
        .await
        .map_err(cmd_err)?;
    providers::set_api_key_ref(&state.db.pool, &provider_id, None)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn delete_provider(state: State<'_, AppState>, provider_id: String) -> CmdResult<()> {
    state
        .secrets
        .delete(&provider_account(&provider_id))
        .await
        .map_err(cmd_err)?;
    providers::delete(&state.db.pool, &provider_id)
        .await
        .map_err(cmd_err)?;
    providers::clear_conversation_refs(&state.db.pool, &provider_id)
        .await
        .map_err(cmd_err)?;
    if settings::get(&state.db.pool, "last_provider_id")
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some(provider_id.as_str())
    {
        let _ = settings::delete(&state.db.pool, "last_provider_id").await;
    }
    Ok(())
}

async fn fetch_models_impl(state: &AppState, provider_id: &str) -> CmdResult<Vec<String>> {
    let provider = providers::get(&state.db.pool, provider_id)
        .await
        .map_err(cmd_err)?;
    let kind =
        ProviderKind::parse(&provider.kind).ok_or_else(|| "Unknown provider kind".to_string())?;
    let account = provider
        .api_key_ref
        .clone()
        .unwrap_or_else(|| provider.id.clone());
    let key = state.secrets.get(&account).await.map_err(cmd_err)?;
    let models = llm::fetch_models(kind, &provider.base_url, key.as_deref())
        .await
        .map_err(cmd_err)?;
    providers::set_models(&state.db.pool, provider_id, &models)
        .await
        .map_err(cmd_err)?;
    Ok(models)
}

#[tauri::command]
pub async fn fetch_models(
    state: State<'_, AppState>,
    provider_id: String,
) -> CmdResult<Vec<String>> {
    fetch_models_impl(state.inner(), &provider_id).await
}

#[tauri::command]
pub async fn fetch_models_for_config(
    state: State<'_, AppState>,
    kind: String,
    base_url: String,
    api_key: Option<String>,
    provider_id: Option<String>,
) -> CmdResult<Vec<String>> {
    let kind = ProviderKind::parse(&kind).ok_or_else(|| "Unknown provider kind".to_string())?;
    let url = Url::parse(base_url.trim().trim_end_matches('/'))
        .map_err(|_| "Invalid base URL".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" => {
            let host = url.host_str().unwrap_or("");
            if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
                return Err("HTTP is only allowed for localhost providers".into());
            }
        }
        _ => return Err("Base URL must use https://".into()),
    }
    let key = match api_key.filter(|key| !key.trim().is_empty()) {
        Some(key) => Some(key),
        None => {
            if let Some(provider_id) = provider_id.as_deref() {
                let provider = providers::get(&state.db.pool, provider_id)
                    .await
                    .map_err(cmd_err)?;
                let account = provider
                    .api_key_ref
                    .clone()
                    .unwrap_or_else(|| provider.id.clone());
                state.secrets.get(&account).await.map_err(cmd_err)?
            } else {
                None
            }
        }
    };
    llm::fetch_models(kind, url.as_str(), key.as_deref())
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn test_provider(state: State<'_, AppState>, provider_id: String) -> CmdResult<KeyTest> {
    let models = fetch_models_impl(state.inner(), &provider_id).await?;
    let message = if models.is_empty() {
        "Connected, but the endpoint returned no models.".into()
    } else {
        format!("Connected. {} model(s) available.", models.len())
    };
    Ok(KeyTest {
        ok: true,
        models,
        message,
    })
}
