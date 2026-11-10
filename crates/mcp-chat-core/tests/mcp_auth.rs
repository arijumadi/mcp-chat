use std::sync::Arc;

use mcp_chat_core::db::Db;
use mcp_chat_core::db::repo::servers;
use mcp_chat_core::mcp::McpManager;
use mcp_chat_core::secrets::{MemoryStore, SecretStore, server_api_key_account};
use mcp_mock::mcp::{self, McpMode};

#[tokio::test]
async fn api_key_header_is_sent_from_the_keychain() {
    let mock = mcp::start_with_api_key(McpMode::Modern, "secret-key").await;
    let db = Db::open_memory().await.unwrap();
    let secrets = Arc::new(MemoryStore::default());
    let server = servers::create(
        &db.pool,
        &servers::ServerConfig::http(
            "IDX",
            "idx",
            &mock.url,
            "streamable_http",
            "api_key",
            "{}",
            "ask",
        ),
    )
    .await
    .unwrap();
    secrets
        .set(&server_api_key_account(&server.id), "secret-key")
        .await
        .unwrap();
    let manager = McpManager::new(db.clone(), secrets.clone());
    let tools = manager.refresh_tools(&server.id).await.unwrap();
    assert!(tools.iter().any(|tool| tool.name == "echo"));

    let without = servers::create(
        &db.pool,
        &servers::ServerConfig::http(
            "IDX missing",
            "idx_missing",
            &mock.url,
            "streamable_http",
            "api_key",
            "{}",
            "ask",
        ),
    )
    .await
    .unwrap();
    let error = manager.refresh_tools(&without.id).await.unwrap_err();
    assert!(error.user_message().contains("X-API-Key"));
}
