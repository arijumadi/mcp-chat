use mcp_chat_core::llm;
use mcp_chat_core::models::ProviderKind;
use mcp_mock::provider;

#[tokio::test]
async fn fetches_models_for_both_provider_kinds() {
    let mock = provider::start().await;
    let openai = llm::fetch_models(ProviderKind::OpenaiCompat, &mock.url, None)
        .await
        .unwrap();
    assert!(openai.contains(&"mock-text".to_string()));
    let anthropic = llm::fetch_models(ProviderKind::Anthropic, &mock.url, None)
        .await
        .unwrap();
    assert!(anthropic.contains(&"mock-tool".to_string()));
}
