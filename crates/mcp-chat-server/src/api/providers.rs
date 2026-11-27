//! Model providers.
//!
//! Ported from `src-tauri/src/commands/providers.rs`. The validation rules are
//! security-relevant (only https, or http on loopback), so they are copied
//! rather than simplified.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use mcp_chat_core::db::repo::{providers, settings};
use mcp_chat_core::llm;
use mcp_chat_core::models::{ProviderKind, ProviderView};
use mcp_chat_core::secrets::provider_account;
use mcp_chat_core::util::redact_secret;

use super::{err, parse, to_value};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInput {
    name: String,
    kind: String,
    base_url: String,
    model: String,
    #[serde(default)]
    context_window: Option<i64>,
    #[serde(default)]
    max_output_tokens: Option<i64>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    models: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyTest {
    ok: bool,
    models: Vec<String>,
    message: String,
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

pub async fn list_providers(state: &AppState, _args: Value) -> Result<Value, String> {
    let list = providers::list(&state.db.pool).await.map_err(err)?;
    let mut out = Vec::new();
    for provider in list {
        out.push(view(state, provider).await);
    }
    to_value(out)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProvider {
    input: ProviderInput,
}

pub async fn create_provider(state: &AppState, args: Value) -> Result<Value, String> {
    let params: CreateProvider = parse(args)?;
    let input = params.input;
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
    .map_err(err)?;
    if let Some(key) = input
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
    {
        state
            .secrets
            .set(&provider_account(&provider.id), key.trim())
            .await
            .map_err(err)?;
        providers::set_api_key_ref(&state.db.pool, &provider.id, Some(&provider.id))
            .await
            .map_err(err)?;
    }
    if let Some(models) = &input.models {
        providers::set_models(&state.db.pool, &provider.id, models)
            .await
            .map_err(err)?;
    }
    let provider = providers::get(&state.db.pool, &provider.id)
        .await
        .map_err(err)?;
    to_value(view(state, provider).await)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProvider {
    provider_id: String,
    input: ProviderInput,
}

pub async fn update_provider(state: &AppState, args: Value) -> Result<Value, String> {
    let params: UpdateProvider = parse(args)?;
    let input = params.input;
    validate(&input)?;
    let provider = providers::update(
        &state.db.pool,
        &params.provider_id,
        input.name.trim(),
        &input.kind,
        input.base_url.trim().trim_end_matches('/'),
        input.model.trim(),
        input.context_window.unwrap_or(128_000),
        input.max_output_tokens.unwrap_or(8_192),
    )
    .await
    .map_err(err)?;
    if let Some(key) = input
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
    {
        state
            .secrets
            .set(&provider_account(&provider.id), key.trim())
            .await
            .map_err(err)?;
        providers::set_api_key_ref(&state.db.pool, &provider.id, Some(&provider.id))
            .await
            .map_err(err)?;
    }
    if let Some(models) = &input.models {
        providers::set_models(&state.db.pool, &provider.id, models)
            .await
            .map_err(err)?;
    }
    let provider = providers::get(&state.db.pool, &params.provider_id)
        .await
        .map_err(err)?;
    to_value(view(state, provider).await)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderId {
    provider_id: String,
}

pub async fn delete_provider(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ProviderId = parse(args)?;
    state
        .secrets
        .delete(&provider_account(&params.provider_id))
        .await
        .map_err(err)?;
    providers::delete(&state.db.pool, &params.provider_id)
        .await
        .map_err(err)?;
    providers::clear_conversation_refs(&state.db.pool, &params.provider_id)
        .await
        .map_err(err)?;
    if settings::get(&state.db.pool, "last_provider_id")
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some(params.provider_id.as_str())
    {
        let _ = settings::delete(&state.db.pool, "last_provider_id").await;
    }
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetProviderKey {
    provider_id: String,
    api_key: String,
}

pub async fn set_provider_key(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetProviderKey = parse(args)?;
    if params.api_key.trim().is_empty() {
        return Err("API key is empty".into());
    }
    state
        .secrets
        .set(
            &provider_account(&params.provider_id),
            params.api_key.trim(),
        )
        .await
        .map_err(err)?;
    providers::set_api_key_ref(
        &state.db.pool,
        &params.provider_id,
        Some(&params.provider_id),
    )
    .await
    .map_err(err)?;
    let provider = providers::get(&state.db.pool, &params.provider_id)
        .await
        .map_err(err)?;
    to_value(view(state, provider).await)
}

pub async fn clear_provider_key(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ProviderId = parse(args)?;
    state
        .secrets
        .delete(&provider_account(&params.provider_id))
        .await
        .map_err(err)?;
    providers::set_api_key_ref(&state.db.pool, &params.provider_id, None)
        .await
        .map_err(err)?;
    to_value(())
}

async fn fetch_models_impl(state: &AppState, provider_id: &str) -> Result<Vec<String>, String> {
    let provider = providers::get(&state.db.pool, provider_id)
        .await
        .map_err(err)?;
    let kind =
        ProviderKind::parse(&provider.kind).ok_or_else(|| "Unknown provider kind".to_string())?;
    let account = provider
        .api_key_ref
        .clone()
        .unwrap_or_else(|| provider.id.clone());
    let key = state.secrets.get(&account).await.map_err(err)?;
    let models = llm::fetch_models(kind, &provider.base_url, key.as_deref())
        .await
        .map_err(err)?;
    providers::set_models(&state.db.pool, provider_id, &models)
        .await
        .map_err(err)?;
    Ok(models)
}

pub async fn fetch_models(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ProviderId = parse(args)?;
    let models = fetch_models_impl(state, &params.provider_id).await?;
    to_value(models)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchModelsForConfig {
    kind: String,
    base_url: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
}

pub async fn fetch_models_for_config(state: &AppState, args: Value) -> Result<Value, String> {
    let params: FetchModelsForConfig = parse(args)?;
    let kind =
        ProviderKind::parse(&params.kind).ok_or_else(|| "Unknown provider kind".to_string())?;
    let url = Url::parse(params.base_url.trim().trim_end_matches('/'))
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
    let key = match params.api_key.filter(|key| !key.trim().is_empty()) {
        Some(key) => Some(key),
        None => {
            if let Some(provider_id) = params.provider_id.as_deref() {
                let provider = providers::get(&state.db.pool, provider_id)
                    .await
                    .map_err(err)?;
                let account = provider
                    .api_key_ref
                    .clone()
                    .unwrap_or_else(|| provider.id.clone());
                state.secrets.get(&account).await.map_err(err)?
            } else {
                None
            }
        }
    };
    let models = llm::fetch_models(kind, url.as_str(), key.as_deref())
        .await
        .map_err(err)?;
    to_value(models)
}

pub async fn test_provider(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ProviderId = parse(args)?;
    let models = fetch_models_impl(state, &params.provider_id).await?;
    let message = if models.is_empty() {
        "Connected, but the endpoint returned no models.".into()
    } else {
        format!("Connected. {} model(s) available.", models.len())
    };
    to_value(KeyTest {
        ok: true,
        models,
        message,
    })
}
