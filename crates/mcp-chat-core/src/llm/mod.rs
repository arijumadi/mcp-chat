pub mod anthropic;
pub mod openai;
pub mod provider;
pub mod sse;

use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::Result;
use crate::models::ProviderKind;

pub use provider::{
    ChatMessage, ChatRequest, ChatRole, ContentBlock, Delta, DeltaStream, FinishReason, Provider,
    ToolChoice, ToolDef,
};

pub fn provider_for(kind: ProviderKind) -> Arc<dyn Provider> {
    match kind {
        ProviderKind::OpenaiCompat => Arc::new(openai::OpenAiProvider),
        ProviderKind::Anthropic => Arc::new(anthropic::AnthropicProvider),
    }
}

pub async fn fetch_models(
    kind: ProviderKind,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>> {
    let client = provider::http_client()?;
    let url = provider::models_url(kind, base_url);
    let mut builder = client.get(&url).header("Accept", "application/json");
    match kind {
        ProviderKind::OpenaiCompat => {
            if let Some(key) = api_key {
                builder = builder.bearer_auth(key);
            }
        }
        ProviderKind::Anthropic => {
            if let Some(key) = api_key {
                builder = builder.header("x-api-key", key);
            }
            builder = builder.header("anthropic-version", "2023-06-01");
        }
    }
    let response = builder.send().await?;
    if !response.status().is_success() {
        return Err(provider::response_error(response).await);
    }
    let value: Value = response.json().await?;
    let mut models = value
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("id")
                        .or_else(|| item.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    models.sort();
    models.dedup();
    Ok(models)
}

/// Run a non-streaming request by draining a stream. Used by the compactor and
/// by tests; chat turns always go through the agent loop.
pub async fn complete(
    kind: ProviderKind,
    mut req: ChatRequest,
    cancel: CancellationToken,
) -> Result<String> {
    // Keep the caller's tool catalog so prefix caching can reuse the same
    // tools block; disallow calls instead of removing the definitions.
    req.tool_choice = ToolChoice::None;
    let provider = provider_for(kind);
    let mut stream = provider.stream(req, cancel).await?;
    use futures::StreamExt;
    let mut out = String::new();
    while let Some(item) = stream.next().await {
        match item? {
            Delta::Text(text) => out.push_str(&text),
            Delta::Done(_) => break,
            _ => {}
        }
    }
    Ok(out)
}
