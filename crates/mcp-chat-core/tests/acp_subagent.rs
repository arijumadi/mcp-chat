//! End-to-end test for an ACP child conversation.
//!
//! Unlike `acp_multi_turn.rs`, which exercises the protocol client alone, this
//! runs the whole engine path: the `subagent` tool with an `agent` slug, a real
//! ACP process, part rows in the child transcript, the parent's `<subagent>`
//! report, and the turn guard that keeps the LLM loop out of an ACP child.

use std::sync::Arc;
use std::time::Duration;

use mcp_chat_core::agent::ChatEngine;
use mcp_chat_core::db::Db;
use mcp_chat_core::db::repo::acp_agents::AcpAgentConfig;
use mcp_chat_core::db::repo::{acp_agents, conversations, messages};
use mcp_chat_core::mcp::McpManager;
use mcp_chat_core::secrets::MemoryStore;
use serde_json::json;

struct Harness {
    engine: Arc<ChatEngine>,
    db: Db,
    parent_id: String,
    _tmp: tempfile::TempDir,
}

impl Harness {
    async fn new() -> Harness {
        let db = Db::open_memory().await.unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let mcp = Arc::new(McpManager::new(db.clone(), secrets.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let engine = ChatEngine::new(
            db.clone(),
            secrets,
            mcp,
            tmp.path().join("tool-output"),
            tmp.path().join("attachments"),
        );
        let parent = conversations::create(&db.pool, "Parent", None, None)
            .await
            .unwrap();
        conversations::set_workspace(&db.pool, &parent.id, Some(workspace.to_str().unwrap()))
            .await
            .unwrap();
        Harness {
            engine,
            db,
            parent_id: parent.id,
            _tmp: tmp,
        }
    }

    /// Register the mock agent binary under a slug.
    async fn register_agent(&self, slug: &str, scenario: &str) {
        let config = AcpAgentConfig {
            name: "Mock ACP".into(),
            slug: slug.into(),
            command: env!("CARGO_BIN_EXE_mock-acp-agent").to_owned(),
            args_json: json!([scenario]).to_string(),
            env_json: "{}".into(),
            permission_policy: "ask".into(),
            enabled: true,
        };
        acp_agents::create(&self.db.pool, &config).await.unwrap();
    }

    /// Insert a tool_call part so the engine can mark it, like a real turn.
    async fn parent_tool_part(&self) -> (String, String) {
        let message = messages::create(&self.db.pool, &self.parent_id, "assistant", "streaming")
            .await
            .unwrap();
        let part = messages::append_raw_part(
            &self.db.pool,
            &message.id,
            "tool_call",
            &json!({
                "callId": "call-1",
                "name": "subagent",
                "server": "builtin",
                "args": { "description": "test" },
                "state": "running",
                "executed": true,
            }),
        )
        .await
        .unwrap();
        (message.id, part.id)
    }
}

#[tokio::test]
async fn an_acp_subagent_runs_and_reports_back() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "echo").await;
    let (_message_id, part_id) = harness.parent_tool_part().await;

    let (child_id, content) = harness
        .engine
        .launch_subagent(
            &harness.parent_id,
            "ACP check",
            "say hello",
            None,
            Some("mock"),
            &part_id,
        )
        .await
        .expect("launch");
    assert!(content.contains("ACP subagent"), "{content}");

    // The child must be marked as ACP, and it must not have a provider: the
    // agent owns its own model.
    let child = conversations::get(&harness.db.pool, &child_id)
        .await
        .unwrap();
    assert_eq!(child.parent_id.as_deref(), Some(harness.parent_id.as_str()));
    assert_eq!(child.acp_agent.as_deref(), Some("mock"));
    assert!(
        child.provider_id.is_none(),
        "ACP child must not inherit a provider"
    );

    // Wait for the supervisor to deliver the report.
    let report = wait_for_report(&harness, 40).await;
    assert!(report.contains("<subagent"), "{report}");
    assert!(report.contains("state=\"completed\""), "{report}");
    // The mock echoes its prompt, so the task text proves the prompt arrived.
    assert!(report.contains("say hello"), "{report}");
}

#[tokio::test]
async fn an_acp_child_refuses_a_direct_message() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "echo").await;
    let (_message_id, part_id) = harness.parent_tool_part().await;
    let (child_id, _) = harness
        .engine
        .launch_subagent(
            &harness.parent_id,
            "ACP check",
            "say hello",
            None,
            Some("mock"),
            &part_id,
        )
        .await
        .unwrap();
    let _ = wait_for_report(&harness, 40).await;

    // ACP children are driven by the agent, so a direct send must be refused
    // rather than silently starting the LLM loop.
    let error = harness
        .engine
        .send_message(&child_id, "direct")
        .await
        .expect_err("ACP child must refuse a direct message");
    assert!(
        error.user_message().contains("ACP agent"),
        "{}",
        error.user_message()
    );

    // And the refusal must not have written a message.
    let history = messages::list_for_conversation(&harness.db.pool, &child_id)
        .await
        .unwrap();
    assert!(
        !history
            .iter()
            .any(|item| matches!(&item.parts[..], [part] if part.payload()["text"] == "direct")),
        "a refused send must leave no message behind"
    );
}

#[tokio::test]
async fn an_unknown_slug_is_reported_clearly() {
    let harness = Harness::new().await;
    let (_message_id, part_id) = harness.parent_tool_part().await;
    let error = harness
        .engine
        .launch_subagent(
            &harness.parent_id,
            "ACP check",
            "say hello",
            None,
            Some("does-not-exist"),
            &part_id,
        )
        .await
        .expect_err("unknown slug must fail");
    assert!(
        error.user_message().contains("does-not-exist"),
        "{}",
        error.user_message()
    );
    assert!(
        error.user_message().contains("ACP agents"),
        "the message should say where to look: {}",
        error.user_message()
    );
}

/// Poll the parent history until the `<subagent>` report shows up.
async fn wait_for_report(harness: &Harness, attempts: usize) -> String {
    for _ in 0..attempts {
        let history = messages::list_for_conversation(&harness.db.pool, &harness.parent_id)
            .await
            .unwrap();
        for item in history {
            if item.message.role != "user" || item.message.status != "synthetic" {
                continue;
            }
            let text = mcp_chat_core::agent::context::text_parts(&item.parts);
            if text.contains("<subagent") {
                return text;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no subagent report arrived");
}
