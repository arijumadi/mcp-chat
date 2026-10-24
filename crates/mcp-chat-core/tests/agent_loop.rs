use std::sync::Arc;
use std::time::Duration;

use mcp_chat_core::agent::approvals::{ApprovalAnswer, ApprovalDecision, InputAnswer};
use mcp_chat_core::agent::{ChatEngine, runner};
use mcp_chat_core::db::Db;
use mcp_chat_core::db::repo::{conversations, messages, permissions, providers, servers};
use mcp_chat_core::events::{CoreEvent, TurnStatus};
use mcp_chat_core::mcp::McpManager;
use mcp_chat_core::models::PartPayload;
use mcp_chat_core::secrets::{MemoryStore, SecretStore};
use mcp_mock::mcp::{self, McpMode};
use mcp_mock::provider;
use serde_json::json;

struct Harness {
    engine: Arc<ChatEngine>,
    db: Db,
    conversation_id: String,
    provider: provider::MockProvider,
    _tmp: tempfile::TempDir,
}

impl Harness {
    async fn new(model: &str, server_policy: Option<&str>, seed_messages: usize) -> Harness {
        Self::new_kind(model, "openai_compat", server_policy, seed_messages).await
    }

    async fn new_kind(
        model: &str,
        kind: &str,
        server_policy: Option<&str>,
        seed_messages: usize,
    ) -> Harness {
        let db = Db::open_memory().await.unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let provider_mock = provider::start().await;
        let mcp = mcp::start(McpMode::Modern).await;

        let provider = providers::create(
            &db.pool,
            "Mock",
            kind,
            &provider_mock.url,
            model,
            None,
            128_000,
            4096,
        )
        .await
        .unwrap();
        providers::set_api_key_ref(&db.pool, &provider.id, Some(&provider.id))
            .await
            .unwrap();
        secrets.set(&provider.id, "test-key").await.unwrap();

        let conversation = conversations::create(&db.pool, "Test", Some(&provider.id), Some(model))
            .await
            .unwrap();
        conversations::set_tool_mode(&db.pool, &conversation.id, "direct")
            .await
            .unwrap();

        if let Some(policy) = server_policy {
            servers::create(
                &db.pool,
                "Mock MCP",
                "mock",
                &mcp.url,
                "streamable_http",
                "none",
                "{}",
                policy,
            )
            .await
            .unwrap();
        }

        for i in 0..seed_messages {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            let message = messages::create(&db.pool, &conversation.id, role, "completed")
                .await
                .unwrap();
            messages::append_part(
                &db.pool,
                &message.id,
                "text",
                &PartPayload::Text {
                    text: format!("seeded message {i} with enough words to matter"),
                },
            )
            .await
            .unwrap();
        }

        let manager = Arc::new(McpManager::new(db.clone(), secrets.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let engine = ChatEngine::new(db.clone(), secrets, manager, tmp.path().join("tool-output"));

        Harness {
            engine,
            db,
            conversation_id: conversation.id,
            provider: provider_mock,
            _tmp: tmp,
        }
    }

    async fn send_and_wait(
        &self,
        text: &str,
        approval: ApprovalDecision,
        feedback: Option<&str>,
        input_name: &str,
    ) -> Vec<CoreEvent> {
        let mut receiver = self.engine.subscribe();
        self.engine
            .send_message(&self.conversation_id, text)
            .await
            .unwrap();
        let mut events = Vec::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(31), receiver.recv())
                .await
                .expect("turn timed out")
                .expect("event channel");
            match &event {
                CoreEvent::Status {
                    status: TurnStatus::Idle | TurnStatus::Error,
                    ..
                } => {
                    events.push(event);
                    break;
                }
                CoreEvent::Approval { call_id, .. } => {
                    self.engine.respond_approval(
                        call_id,
                        ApprovalAnswer {
                            decision: approval,
                            feedback: feedback.map(str::to_owned),
                        },
                    );
                }
                CoreEvent::InputRequest { call_id, .. } => {
                    self.engine.respond_input(
                        call_id,
                        InputAnswer {
                            responses: json!({
                                "answer": {
                                    "action": "accept",
                                    "content": {"name": input_name}
                                }
                            }),
                            cancelled: false,
                        },
                    );
                }
                _ => {}
            }
            events.push(event);
        }
        events
    }

    async fn history(&self) -> Vec<mcp_chat_core::models::MessageWithParts> {
        messages::list_for_conversation(&self.db.pool, &self.conversation_id)
            .await
            .unwrap()
    }

    async fn assistant_text(&self) -> String {
        self.history()
            .await
            .iter()
            .filter(|message| message.message.role == "assistant")
            .flat_map(|message| message.parts.iter())
            .filter(|part| part.kind == "text")
            .filter_map(|part| {
                part.payload()
                    .get("text")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

#[tokio::test]
async fn streams_text_and_persists_it() {
    let h = Harness::new("mock-text", None, 0).await;
    let events = h
        .send_and_wait("hello", ApprovalDecision::Once, None, "ari")
        .await;
    assert!(events.iter().any(|event| matches!(
        event,
        CoreEvent::Status {
            status: TurnStatus::Idle,
            ..
        }
    )));
    assert!(
        h.assistant_text()
            .await
            .contains("Hello from the mock provider.")
    );
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| { tool["function"]["name"] == "read_tool_output" })
    );
}

#[tokio::test]
async fn runs_a_tool_call_end_to_end() {
    let h = Harness::new("mock-tool", Some("auto"), 0).await;
    h.send_and_wait("use echo", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let tool_part = history
        .iter()
        .flat_map(|message| message.parts.iter())
        .find(|part| part.kind == "tool_call")
        .expect("tool part");
    let payload = tool_part.payload();
    assert_eq!(payload["state"], "completed");
    assert_eq!(payload["executed"], true);
    assert!(
        payload["resultPreview"]
            .as_str()
            .unwrap()
            .contains("echo: hello")
    );

    let tool_message = history
        .iter()
        .find(|message| message.message.role == "tool")
        .expect("tool result message");
    let result = tool_message.parts[0].payload();
    assert_eq!(result["ok"], true);
    assert_eq!(result["content"], "echo: hello");

    let requests = h.provider.requests();
    assert_eq!(requests.len(), 2);
    let second = &requests[1];
    let tool_message = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("tool message in provider request");
    assert_eq!(tool_message["content"], "echo: hello");
    let echo_schema = second["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| {
            tool["function"]["name"]
                .as_str()
                .unwrap()
                .ends_with("__echo")
        })
        .unwrap()["function"]["parameters"]
        .clone();
    assert_eq!(echo_schema["properties"]["text"]["type"], "string");
}

#[tokio::test]
async fn parallel_tools_keep_call_order() {
    let h = Harness::new("mock-parallel", Some("auto"), 0).await;
    h.send_and_wait("two tools", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let results = history
        .iter()
        .filter(|message| message.message.role == "tool")
        .flat_map(|message| message.parts.iter())
        .map(|part| part.payload())
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert!(
        results[0]["content"]
            .as_str()
            .unwrap()
            .contains("echo: one")
    );
    assert!(results[1]["content"].as_str().unwrap().contains("sum"));
}

#[tokio::test]
async fn unknown_and_invalid_tools_settle_as_errors() {
    let unknown = Harness::new("mock-unknown", Some("auto"), 0).await;
    unknown
        .send_and_wait("unknown", ApprovalDecision::Once, None, "ari")
        .await;
    let history = unknown.history().await;
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .unwrap()
        .parts[0]
        .payload();
    assert!(result["ok"] == false);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("No tool named")
    );

    let bad = Harness::new("mock-badargs", Some("auto"), 0).await;
    bad.send_and_wait("bad args", ApprovalDecision::Once, None, "ari")
        .await;
    let history = bad.history().await;
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .unwrap()
        .parts[0]
        .payload();
    assert!(result["ok"] == false);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("Invalid tool input")
    );
}

#[tokio::test]
async fn output_limit_continues_then_stops() {
    let h = Harness::new("mock-length", None, 0).await;
    h.send_and_wait("long answer", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let assistant = history
        .iter()
        .filter(|message| message.message.role == "assistant")
        .count();
    assert_eq!(assistant, 2);
    let synthetic = history
        .iter()
        .filter(|message| message.message.status == "synthetic")
        .count();
    assert_eq!(synthetic, 1);
    assert!(h.assistant_text().await.contains("Second piece."));
}

#[tokio::test]
async fn interrupted_stream_continues_with_partial_history() {
    let h = Harness::new("mock-cut", None, 0).await;
    h.send_and_wait("cut", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let assistants = history
        .iter()
        .filter(|message| message.message.role == "assistant")
        .collect::<Vec<_>>();
    assert_eq!(assistants.len(), 2);
    assert_eq!(assistants[0].message.status, "interrupted");
    assert!(
        h.assistant_text()
            .await
            .contains("Continued after interruption.")
    );
}

#[tokio::test]
async fn context_overflow_compacts_and_retries() {
    let h = Harness::new("mock-overflow", None, 10).await;
    h.send_and_wait("overflow now", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let summary = history
        .iter()
        .flat_map(|message| message.parts.iter())
        .find(|part| part.kind == "summary")
        .expect("summary part");
    assert!(
        summary
            .payload()
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap()
            .contains("Summary")
    );
    assert!(h.assistant_text().await.contains("After compaction."));
}

#[tokio::test]
async fn user_rejection_with_feedback_reaches_the_model() {
    let h = Harness::new("mock-tool", Some("ask"), 0).await;
    h.send_and_wait(
        "use echo",
        ApprovalDecision::Reject,
        Some("do not echo this"),
        "ari",
    )
    .await;
    let history = h.history().await;
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .unwrap()
        .parts[0]
        .payload();
    assert!(result["ok"] == false);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("User rejected this call: do not echo this")
    );
    let requests = h.provider.requests();
    let second = &requests[1];
    assert!(
        second["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .unwrap()
                    .contains("do not echo this"))
    );
}

#[tokio::test]
async fn mrtr_elicitation_round_trip() {
    let h = Harness::new("mock-elicit", Some("auto"), 0).await;
    let events = h
        .send_and_wait("ask me", ApprovalDecision::Once, None, "ari")
        .await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CoreEvent::InputRequest { .. }))
    );
    let history = h.history().await;
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .unwrap()
        .parts[0]
        .payload();
    assert_eq!(result["content"], "answer: ari");
}

#[tokio::test]
async fn progress_notifications_update_the_card() {
    let h = Harness::new("mock-slow", Some("auto"), 0).await;
    let events = h
        .send_and_wait("slow", ApprovalDecision::Once, None, "ari")
        .await;
    let progress = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                CoreEvent::Tool {
                    progress: Some(_),
                    ..
                }
            )
        })
        .count();
    assert!(progress >= 3, "expected progress events, got {progress}");
}

#[tokio::test]
async fn large_output_spills_and_can_be_paged() {
    let h = Harness::new("mock-big", Some("auto"), 0).await;
    h.send_and_wait("big", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let part = history
        .iter()
        .flat_map(|message| message.parts.iter())
        .find(|part| part.kind == "tool_call")
        .unwrap();
    let payload = part.payload();
    assert_eq!(payload["truncated"], true);
    assert!(payload["fullResultRef"].as_str().is_some());
    let call_id = payload["callId"].as_str().unwrap();
    let page = h.engine.read_tool_output(call_id, 0).await.unwrap();
    let page: serde_json::Value = serde_json::from_str(&page).unwrap();
    assert_eq!(page["total"], 50000);
    assert_eq!(page["content"].as_str().unwrap().len(), 50000);
}

#[tokio::test]
async fn unknown_finish_reason_does_not_loop() {
    let h = Harness::new("mock-weird-finish", None, 0).await;
    h.send_and_wait("weird", ApprovalDecision::Once, None, "ari")
        .await;
    assert!(h.assistant_text().await.contains("Odd finish."));
    assert_eq!(h.provider.requests().len(), 1);
}

#[tokio::test]
async fn denied_server_policy_hides_tools() {
    let h = Harness::new("mock-tool", Some("deny"), 0).await;
    h.send_and_wait("use echo", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .unwrap()
        .parts[0]
        .payload();
    assert!(result["ok"] == false);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("No tool named")
    );
    let requests = h.provider.requests();
    assert!(
        !requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"]
                .as_str()
                .unwrap()
                .starts_with("mock__"))
    );
}

#[tokio::test]
async fn anthropic_streams_text_and_tools() {
    let text = Harness::new_kind("mock-anthropic-text", "anthropic", None, 0).await;
    text.send_and_wait("hello", ApprovalDecision::Once, None, "ari")
        .await;
    assert!(
        text.assistant_text()
            .await
            .contains("Hello from Anthropic mock.")
    );

    let tools = Harness::new_kind("mock-anthropic-tool", "anthropic", Some("auto"), 0).await;
    tools
        .send_and_wait("use echo", ApprovalDecision::Once, None, "ari")
        .await;
    let history = tools.history().await;
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .unwrap()
        .parts[0]
        .payload();
    assert_eq!(result["content"], "echo: anthropic");
    assert!(
        tools
            .assistant_text()
            .await
            .contains("Anthropic tool complete.")
    );
}

#[tokio::test]
async fn always_allow_persists_a_rule() {
    let h = Harness::new("mock-tool", Some("ask"), 0).await;
    h.send_and_wait("use echo", ApprovalDecision::Always, None, "ari")
        .await;
    let rules = permissions::list(&h.db.pool).await.unwrap();
    assert!(rules.iter().any(|rule| {
        rule.action == "mcp.mock.echo" && rule.effect == "allow" && rule.resource == "*"
    }));
}

#[tokio::test]
async fn max_steps_disables_tools_with_a_result() {
    let h = Harness::new("mock-always-tool", Some("auto"), 0).await;
    sqlx::query("UPDATE conversations SET max_steps = 1 WHERE id = ?1")
        .bind(&h.conversation_id)
        .execute(&h.db.pool)
        .await
        .unwrap();
    h.send_and_wait("loop", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let results = history
        .iter()
        .filter(|message| message.message.role == "tool")
        .flat_map(|message| message.parts.iter())
        .map(|part| part.payload())
        .collect::<Vec<_>>();
    assert!(results.iter().any(|result| {
        result["content"]
            .as_str()
            .unwrap_or("")
            .contains("Tools are disabled")
    }));
    assert!(history.iter().any(|message| {
        message.message.status == "synthetic"
            && message.parts.iter().any(|part| {
                part.payload()["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("maximum number")
            })
    }));
}

#[tokio::test]
async fn invalid_json_arguments_settle_with_an_error() {
    let h = Harness::new("mock-badjson", Some("auto"), 0).await;
    h.send_and_wait("bad json", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .unwrap()
        .parts[0]
        .payload();
    assert!(result["ok"] == false);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("arguments are not valid JSON")
    );
}

#[tokio::test]
async fn retryable_provider_errors_are_retried_with_backoff() {
    let h = Harness::new("mock-flaky", None, 0).await;
    h.send_and_wait("retry", ApprovalDecision::Once, None, "ari")
        .await;
    assert!(h.assistant_text().await.contains("Recovered after retry."));
    assert_eq!(h.provider.requests().len(), 3);
}

#[tokio::test]
async fn stopping_during_a_tool_marks_it_aborted() {
    let h = Harness::new("mock-sleep", Some("auto"), 0).await;
    let mut receiver = h.engine.subscribe();
    h.engine
        .send_message(&h.conversation_id, "sleep")
        .await
        .unwrap();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
            .await
            .expect("tool start timed out")
            .expect("event channel");
        if matches!(
            &event,
            CoreEvent::Tool {
                state,
                ..
            } if state == "running"
        ) || matches!(
            &event,
            CoreEvent::Part {
                kind,
                ..
            } if kind == "tool_call"
        ) {
            break;
        }
    }
    assert!(h.engine.stop(&h.conversation_id));
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
            .await
            .expect("idle timed out")
            .expect("event channel");
        if matches!(
            &event,
            CoreEvent::Status {
                status: TurnStatus::Idle | TurnStatus::Error,
                ..
            }
        ) {
            break;
        }
    }
    let history = h.history().await;
    let assistant = history
        .iter()
        .find(|message| message.message.role == "assistant")
        .expect("assistant message");
    assert_eq!(assistant.message.status, "interrupted");
    let tool = assistant
        .parts
        .iter()
        .find(|part| part.kind == "tool_call")
        .expect("tool part");
    assert_eq!(tool.payload()["state"], "aborted");
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .expect("tool result")
        .parts[0]
        .payload();
    assert!(result["content"].as_str().unwrap().contains("interrupted"));
}

#[tokio::test]
async fn per_conversation_server_selection_hides_tools() {
    let h = Harness::new("mock-tool", Some("auto"), 0).await;
    mcp_chat_core::db::repo::conversation_servers::set(&h.db.pool, &h.conversation_id, &[])
        .await
        .unwrap();
    h.send_and_wait("use echo", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .expect("tool result")
        .parts[0]
        .payload();
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("No tool named")
    );
}

#[tokio::test]
async fn lazy_tool_mode_reads_schema_then_calls_the_target() {
    let h = Harness::new("mock-lazy", Some("auto"), 0).await;
    conversations::set_tool_mode(&h.db.pool, &h.conversation_id, "lazy")
        .await
        .unwrap();
    h.send_and_wait("use lazy tools", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let names = history
        .iter()
        .flat_map(|message| message.parts.iter())
        .filter(|part| part.kind == "tool_call")
        .map(|part| part.payload()["name"].as_str().unwrap_or("").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["mcp_get_tool_schema", "mcp_call_tool"]);
    let target_result = history
        .iter()
        .filter(|message| message.message.role == "tool")
        .flat_map(|message| message.parts.iter())
        .map(|part| part.payload())
        .find(|payload| {
            payload["content"]
                .as_str()
                .unwrap_or("")
                .contains("echo: lazy")
        })
        .expect("target result");
    assert_eq!(target_result["ok"], true);
    assert!(h.assistant_text().await.contains("Lazy tool complete."));

    let first_request_tools = h.provider.requests()[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap_or("").to_owned())
        .collect::<Vec<_>>();
    assert!(first_request_tools.contains(&"mcp_get_tool_schema".to_string()));
    assert!(first_request_tools.contains(&"mcp_call_tool".to_string()));
    assert!(!first_request_tools.contains(&"mcp_search_tools".to_string()));
    assert!(
        !first_request_tools
            .iter()
            .any(|name| name.ends_with("__echo"))
    );
}

#[tokio::test]
async fn manual_compaction_and_context_status() {
    let h = Harness::new("mock-text", None, 10).await;
    let status = runner::context_status(h.engine.clone(), &h.conversation_id)
        .await
        .unwrap();
    assert!(status.tokens > 0);
    assert!(status.window >= 128_000);
    assert!(status.threshold > 0 && status.threshold < status.window);
    assert!(status.keep_tokens >= 1_000);

    runner::compact_now(h.engine.clone(), &h.conversation_id)
        .await
        .unwrap();
    let history = h.history().await;
    assert!(
        history
            .iter()
            .flat_map(|message| message.parts.iter())
            .any(|part| part.kind == "summary")
    );
}

#[tokio::test]
async fn batch_execute_runs_multiple_mcp_tools_in_one_call() {
    let h = Harness::new("mock-batch", Some("auto"), 0).await;
    conversations::set_tool_mode(&h.db.pool, &h.conversation_id, "lazy")
        .await
        .unwrap();
    h.send_and_wait("batch", ApprovalDecision::Once, None, "ari")
        .await;
    let history = h.history().await;
    let tool_part = history
        .iter()
        .flat_map(|message| message.parts.iter())
        .find(|part| part.kind == "tool_call")
        .expect("batch tool part");
    assert_eq!(tool_part.payload()["name"], "mcp_execute");
    let result = history
        .iter()
        .find(|message| message.message.role == "tool")
        .expect("batch result")
        .parts[0]
        .payload();
    assert_eq!(result["ok"], true);
    let content = result["content"].as_str().unwrap();
    assert!(content.contains("mock :: echo"));
    assert!(content.contains("echo: batch"));
    assert!(content.contains("sum"));
    assert!(h.assistant_text().await.contains("Batch complete."));
}

#[tokio::test]
async fn read_tool_output_pages_are_not_recursively_truncated() {
    let h = Harness::new("mock-big-read", Some("auto"), 0).await;
    h.send_and_wait("big read", ApprovalDecision::Once, None, "ari")
        .await;
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 3);
    let paged_content = requests[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .filter_map(|message| message["content"].as_str())
        .max_by_key(|content| content.len())
        .unwrap_or("");
    assert!(
        paged_content.len() > 20_000,
        "read_tool_output page was truncated again: {} chars",
        paged_content.len()
    );
    assert!(h.assistant_text().await.contains("Paged output complete."));
}
