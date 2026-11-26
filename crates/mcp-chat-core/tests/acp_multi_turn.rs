//! Integration test for the ACP session pump.
//!
//! Uses the `mock-acp-agent` binary in this crate so the test exercises the real
//! process, real stdio, and real SDK connection rather than a stub.

use std::path::PathBuf;
use std::sync::Arc;

use mcp_chat_core::acp::manager::AcpManager;
use mcp_chat_core::acp::session::{
    PermissionDecision, PermissionHandler, PermissionRequestInfo, SessionSpec,
};
use mcp_chat_core::acp::AcpEvent;

/// Path to the mock agent built alongside the tests.
fn mock_agent() -> PathBuf {
    // `CARGO_BIN_EXE_<name>` is set for integration tests and points at the
    // freshly built binary.
    PathBuf::from(env!("CARGO_BIN_EXE_mock-acp-agent"))
}

fn no_permissions() -> PermissionHandler {
    Arc::new(|_: PermissionRequestInfo| Box::pin(async { PermissionDecision::Cancel }))
}

fn spec<'a>(
    local_id: &'a str,
    cwd: &'a std::path::Path,
    args: &'a [String],
    scenario: &'a str,
    events: tokio::sync::mpsc::UnboundedSender<AcpEvent>,
) -> (SessionSpec<'a>, Vec<String>) {
    let mut argv = vec![scenario.to_owned()];
    argv.extend_from_slice(args);
    (
        SessionSpec {
            local_id: local_id.to_owned(),
            command: mock_agent().to_str().expect("utf8 path"),
            args: &argv,
            env: &[],
            cwd,
            permissions: no_permissions(),
            events,
        },
        argv,
    )
}

#[tokio::test]
async fn multi_turn_session_reuses_one_process() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let args: Vec<String> = Vec::new();
    let (spec, _argv) = spec(
        "child-1",
        &cwd,
        &args,
        "echo",
        tokio::sync::mpsc::unbounded_channel().0,
    );

    // The event channel is created inside `open`, so build the spec there.
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let spec = SessionSpec { events: events_tx, ..spec };
    let active = manager.open(spec, no_permissions()).await.unwrap();
    assert!(!active.handle.acp_session_id.is_empty());
    assert_eq!(manager.count().await, 1);

    active.handle.prompt("first question").unwrap();
    let first = collect_turn(&mut events).await;
    assert!(first.contains("turn 1 answer: first question"), "{first}");

    active.handle.prompt("second question").unwrap();
    let second = collect_turn(&mut events).await;
    assert!(second.contains("turn 2 answer: second question"), "{second}");

    manager.close("child-1").await.unwrap();
    assert_eq!(manager.count().await, 0);
}

#[tokio::test]
async fn tool_call_update_becomes_a_tool_event() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let args: Vec<String> = Vec::new();
    let (base, _argv) = spec(
        "child-tool",
        &cwd,
        &args,
        "tool",
        tokio::sync::mpsc::unbounded_channel().0,
    );
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let active = manager
        .open(SessionSpec { events: events_tx, ..base }, no_permissions())
        .await
        .unwrap();

    active.handle.prompt("inspect").unwrap();
    let mut transcript = String::new();
    let mut saw_tool = false;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(20), events.recv()).await {
            Ok(Some(AcpEvent::Text(text))) => transcript.push_str(&text),
            Ok(Some(AcpEvent::ToolCall { title, kind, .. })) => {
                saw_tool = true;
                assert!(title.contains("Read"), "{title}");
                assert_eq!(kind.as_deref(), Some("read"));
            }
            Ok(Some(AcpEvent::TurnEnded { .. })) => break,
            Ok(Some(AcpEvent::TurnFailed { message })) => panic!("turn failed: {message}"),
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed early"),
            Err(_) => panic!("timed out waiting for the tool turn"),
        }
    }
    assert!(saw_tool, "no tool call event");
    assert!(transcript.contains("Checking the file first."), "{transcript}");
    manager.close("child-tool").await.unwrap();
}

#[tokio::test]
async fn missing_binary_is_reported_as_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let (events_tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let args: Vec<String> = Vec::new();
    let spec = SessionSpec {
        local_id: "child-missing".into(),
        command: "jarvis-missing-acp-agent",
        args: &args,
        env: &[],
        cwd: &cwd,
        permissions: no_permissions(),
        events: events_tx,
    };
    let error = manager
        .open(spec, no_permissions())
        .await
        .expect_err("missing binary must fail");
    assert!(
        error.user_message().contains("was not found"),
        "{}",
        error.user_message()
    );
    assert_eq!(manager.count().await, 0);
}

#[tokio::test]
async fn a_crashing_agent_reports_a_failed_turn() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let args: Vec<String> = Vec::new();
    let (base, _argv) = spec(
        "child-crash",
        &cwd,
        &args,
        "crash",
        tokio::sync::mpsc::unbounded_channel().0,
    );
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let active = manager
        .open(SessionSpec { events: events_tx, ..base }, no_permissions())
        .await
        .unwrap();

    active.handle.prompt("crash please").unwrap();
    let failure = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(20), events.recv()).await {
            Ok(Some(AcpEvent::TurnFailed { message })) => break message,
            Ok(Some(AcpEvent::TurnEnded { .. })) => panic!("turn should not complete"),
            Ok(Some(_)) => {}
            Ok(None) => break "event channel closed".to_owned(),
            Err(_) => panic!("timed out waiting for the crash"),
        }
    };
    assert!(!failure.is_empty(), "crash message must say something");
    manager.close("child-crash").await.ok();
}

/// Gather assistant text until the turn ends.
async fn collect_turn(events: &mut tokio::sync::mpsc::UnboundedReceiver<AcpEvent>) -> String {
    let mut text = String::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(20), events.recv()).await {
            Ok(Some(AcpEvent::Text(chunk))) => text.push_str(&chunk),
            Ok(Some(AcpEvent::TurnEnded { .. })) => break,
            Ok(Some(AcpEvent::TurnFailed { message })) => panic!("turn failed: {message}"),
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed before the turn ended"),
            Err(_) => panic!("timed out waiting for the turn; got so far: {text}"),
        }
    }
    text
}
