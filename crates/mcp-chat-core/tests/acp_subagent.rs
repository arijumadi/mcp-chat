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
use mcp_chat_core::events::CoreEvent;
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
async fn an_acp_child_accepts_a_message_but_refuses_a_history_edit() {
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

    // A message is fine: the agent runs the turn. What Jarvis must refuse is
    // rewriting the history the agent owns.
    harness
        .engine
        .send_message(&child_id, "direct")
        .await
        .expect("a message is routed to the agent");

    let error = harness
        .engine
        .regenerate(&child_id)
        .await
        .expect_err("regenerate must be refused");
    assert!(
        error.user_message().contains("keeps its own history"),
        "{}",
        error.user_message()
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

#[tokio::test]
async fn a_main_chat_can_be_bound_to_an_acp_agent_and_run_a_turn() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "echo").await;

    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .expect("bind agent");
    let bound = conversations::get(&harness.db.pool, &harness.parent_id)
        .await
        .unwrap();
    assert_eq!(bound.acp_agent.as_deref(), Some("mock"));

    // A normal message is allowed on an ACP chat; the turn runs the agent.
    harness
        .engine
        .send_message(&harness.parent_id, "hello from the main chat")
        .await
        .expect("send");

    let answer = wait_for_assistant_text(&harness, &harness.parent_id, 60).await;
    assert!(answer.contains("hello from the main chat"), "{answer}");

    // The session id must be persisted, or a restart could never resume.
    let after = conversations::get(&harness.db.pool, &harness.parent_id)
        .await
        .unwrap();
    assert!(
        after.acp_session_id.is_some(),
        "the ACP session id must be stored for resume"
    );
}

#[tokio::test]
async fn regenerate_is_refused_on_an_acp_chat() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "echo").await;
    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .unwrap();

    let error = harness
        .engine
        .regenerate(&harness.parent_id)
        .await
        .expect_err("regenerate must be refused");
    assert!(
        error.user_message().contains("keeps its own history"),
        "{}",
        error.user_message()
    );
}

#[tokio::test]
async fn unbinding_an_acp_chat_clears_the_marker() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "echo").await;
    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .unwrap();
    harness
        .engine
        .set_conversation_agent(&harness.parent_id, None)
        .await
        .expect("unbind");

    let conversation = conversations::get(&harness.db.pool, &harness.parent_id)
        .await
        .unwrap();
    assert!(
        conversation.acp_agent.is_none(),
        "unbinding must clear the marker so the next turn uses the LLM again"
    );
}

#[tokio::test]
async fn binding_an_unknown_agent_is_reported() {
    let harness = Harness::new().await;
    let error = harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("nope"))
        .await
        .expect_err("unknown slug must fail");
    assert!(
        error.user_message().contains("nope"),
        "{}",
        error.user_message()
    );
}

/// Poll a conversation for the newest assistant text.
async fn wait_for_assistant_text(
    harness: &Harness,
    conversation_id: &str,
    attempts: usize,
) -> String {
    for _ in 0..attempts {
        let history = messages::list_for_conversation(&harness.db.pool, conversation_id)
            .await
            .unwrap();
        if let Some(last) = history
            .iter()
            .rev()
            .find(|item| item.message.role == "assistant" && item.message.status != "streaming")
        {
            let text = mcp_chat_core::agent::context::text_parts(&last.parts);
            if !text.trim().is_empty() {
                return text;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no assistant answer arrived");
}

#[tokio::test]
async fn probing_a_working_agent_reports_its_name() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "echo").await;
    let agent = acp_agents::get_by_slug(&harness.db.pool, "mock")
        .await
        .unwrap()
        .unwrap();
    let workspace = harness._tmp.path().join("workspace");

    let probe = ChatEngine::probe_acp_agent(&agent, &workspace)
        .await
        .expect("probe should reach the agent");
    assert!(probe.ok, "{}", probe.message);
    assert_eq!(probe.agent_name.as_deref(), Some("mock-acp"));
    assert!(probe.session_id.is_some(), "the probe must open a session");
}

#[tokio::test]
async fn probing_a_missing_binary_explains_what_is_wrong() {
    let harness = Harness::new().await;
    let config = AcpAgentConfig {
        name: "Broken".into(),
        slug: "broken".into(),
        command: "jarvis-missing-acp-agent".into(),
        args_json: "[]".into(),
        env_json: "{}".into(),
        permission_policy: "ask".into(),
        enabled: true,
    };
    let agent = acp_agents::create(&harness.db.pool, &config).await.unwrap();
    let workspace = harness._tmp.path().join("workspace");

    let error = ChatEngine::probe_acp_agent(&agent, &workspace)
        .await
        .expect_err("a missing binary must fail");
    assert!(
        error.user_message().contains("was not found"),
        "{}",
        error.user_message()
    );
}

/// The per-chat Auto button must reach the ACP agent, not only the built-in
/// tools. Before this, the switch looked enabled and changed nothing here.
#[tokio::test]
async fn auto_approve_on_a_chat_applies_to_its_acp_agent() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "permission").await;
    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .unwrap();
    harness.engine.set_auto_approve(&harness.parent_id, true);

    harness
        .engine
        .send_message(&harness.parent_id, "touch a file")
        .await
        .expect("send");

    // Nobody answers the prompt: if the policy were ignored, the turn would sit
    // waiting for an approval that never comes.
    let answer = wait_for_assistant_text(&harness, &harness.parent_id, 80).await;
    assert!(
        answer.contains("permission=selected option=allow-once"),
        "auto mode should approve without asking: {answer}"
    );
}

/// Auto is not a blanket yes: deleting stays reviewed, matching the built-in
/// tools' rule that irreversible actions still ask.
#[tokio::test]
async fn auto_approve_does_not_cover_a_delete() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "permission-delete").await;
    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .unwrap();
    harness.engine.set_auto_approve(&harness.parent_id, true);

    harness
        .engine
        .send_message(&harness.parent_id, "delete a file")
        .await
        .expect("send");

    // The approval must appear instead of being auto-approved. Listen on the
    // event stream, which is what the UI uses to show the prompt.
    let mut events = harness.engine.subscribe();
    let mut saw_approval = false;
    for _ in 0..80 {
        match tokio::time::timeout(std::time::Duration::from_millis(100), events.recv()).await {
            Ok(Ok(CoreEvent::Approval {
                conversation_id,
                tool,
                ..
            })) if conversation_id == harness.parent_id => {
                assert!(tool.contains("Delete"), "unexpected approval: {tool}");
                saw_approval = true;
                break;
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    assert!(
        saw_approval,
        "a delete must still ask the user, even in Auto mode"
    );
}

/// A refusal or a limit must not be reported to the parent as success.
#[tokio::test]
async fn a_refused_turn_is_reported_as_failed() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "stop:refusal").await;
    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .unwrap();
    harness
        .engine
        .send_message(&harness.parent_id, "do something risky")
        .await
        .expect("send");

    let (state, report) = wait_for_state(&harness, &harness.parent_id).await;
    assert_eq!(
        state, "error",
        "a refusal is a failed turn, not a finished one"
    );
    assert!(
        report.contains("refused"),
        "the reason should say why: {report}"
    );
}

#[tokio::test]
async fn a_token_limit_is_reported_as_failed() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "stop:max_tokens").await;
    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .unwrap();
    harness
        .engine
        .send_message(&harness.parent_id, "write a long essay")
        .await
        .expect("send");

    let (state, report) = wait_for_state(&harness, &harness.parent_id).await;
    assert_eq!(state, "error");
    assert!(report.contains("token limit"), "{report}");
}

/// A cancelled turn is `interrupted`, matching the built-in runner, not an
/// error: the user stopped it on purpose.
#[tokio::test]
async fn a_cancelled_turn_is_interrupted_not_failed() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "stop:cancelled").await;
    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .unwrap();
    harness
        .engine
        .send_message(&harness.parent_id, "start something")
        .await
        .expect("send");

    let (state, _report) = wait_for_state(&harness, &harness.parent_id).await;
    assert_eq!(state, "interrupted");
}

/// Wait for the newest assistant turn to settle, returning its status and text.
async fn wait_for_state(harness: &Harness, conversation_id: &str) -> (String, String) {
    for _ in 0..120 {
        let history = messages::list_for_conversation(&harness.db.pool, conversation_id)
            .await
            .unwrap();
        if let Some(last) = history
            .iter()
            .rev()
            .find(|item| item.message.role == "assistant" && item.message.status != "streaming")
        {
            let text = last
                .message
                .error
                .clone()
                .unwrap_or_else(|| mcp_chat_core::agent::context::text_parts(&last.parts));
            return (last.message.status.clone(), text);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the turn never settled");
}

/// The model, thinking level and mode come from `session/new`. Binding a chat
/// used to only write the marker, so the pickers stayed empty until the first
/// message created the session.
///
/// The options arrive asynchronously now that the switch does not block on the
/// agent starting, so this waits for them rather than reading once.
#[tokio::test]
async fn binding_a_chat_makes_its_options_available_without_a_message() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "echo").await;

    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .expect("bind");

    let mut options = Vec::new();
    for _ in 0..100 {
        options = harness.engine.agent_config(&harness.parent_id).await;
        if !options.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        !options.is_empty(),
        "the pickers need the agent's options before the first message"
    );
    let model = options.iter().find(|option| option.id == "model").unwrap();
    assert_eq!(model.current_value, "mock-model-a");
    assert!(options.iter().any(|option| option.id == "mode"));
}

/// Binding must survive an agent that cannot start: the chat keeps the binding
/// and the failure shows up on the turn, where the user can see it.
#[tokio::test]
async fn binding_a_broken_agent_keeps_the_binding() {
    let harness = Harness::new().await;
    let config = AcpAgentConfig {
        name: "Broken".into(),
        slug: "broken".into(),
        command: "jarvis-missing-acp-agent".into(),
        args_json: "[]".into(),
        env_json: "{}".into(),
        permission_policy: "ask".into(),
        enabled: true,
    };
    acp_agents::create(&harness.db.pool, &config).await.unwrap();

    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("broken"))
        .await
        .expect("binding should not fail on a missing binary");

    let conversation = conversations::get(&harness.db.pool, &harness.parent_id)
        .await
        .unwrap();
    assert_eq!(conversation.acp_agent.as_deref(), Some("broken"));
    assert!(
        harness
            .engine
            .agent_config(&harness.parent_id)
            .await
            .is_empty(),
        "a broken agent exposes no options"
    );
}

/// Switching must not wait for the agent to start. A real Antigravity session
/// takes 1.5-2.4s to answer `session/new`, and blocking a visual change on that
/// felt like a hang.
#[tokio::test]
async fn switching_returns_before_the_agent_is_ready() {
    let harness = Harness::new().await;
    harness.register_agent("mock", "echo").await;

    let started = std::time::Instant::now();
    harness
        .engine
        .set_conversation_agent(&harness.parent_id, Some("mock"))
        .await
        .expect("bind");
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "the switch blocked for {elapsed:?}; opening the agent must not be awaited"
    );

    // The binding is already visible to the UI.
    let conversation = conversations::get(&harness.db.pool, &harness.parent_id)
        .await
        .unwrap();
    assert_eq!(conversation.acp_agent.as_deref(), Some("mock"));

    // The options still arrive, without sending a message.
    let mut ready = Vec::new();
    for _ in 0..80 {
        ready = harness.engine.agent_config(&harness.parent_id).await;
        if !ready.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        !ready.is_empty(),
        "the options must still arrive in the background"
    );
}

/// Reported: after a few switches the pickers went empty and the transcript
/// stopped, because the session cap was reached by leaked processes.
///
/// `ensure_session` keys on the conversation id alone, so switching agents used
/// to reuse the old session and leave its process running. Four switches filled
/// the cap.
#[tokio::test]
async fn switching_agents_does_not_leak_sessions() {
    let harness = Harness::new().await;
    harness.register_agent("first", "echo").await;
    harness.register_agent("second", "echo").await;

    for _ in 0..3 {
        harness
            .engine
            .set_conversation_agent(&harness.parent_id, Some("first"))
            .await
            .expect("bind first");
        wait_for_sessions(&harness, 1).await;
        harness
            .engine
            .set_conversation_agent(&harness.parent_id, Some("second"))
            .await
            .expect("bind second");
        wait_for_sessions(&harness, 1).await;
    }

    assert_eq!(
        harness.engine.acp.count().await,
        1,
        "each switch must replace the session, not add one"
    );

    harness.engine.close_acp_session(&harness.parent_id).await;
    assert_eq!(harness.engine.acp.count().await, 0);
}

/// Waiting for the background open to finish without depending on its timing.
async fn wait_for_sessions(harness: &Harness, want: usize) {
    for _ in 0..100 {
        if harness.engine.acp.count().await == want {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
