//! Integration test for the ACP session pump.
//!
//! Uses the `mock-acp-agent` binary in this crate, so the test exercises the
//! real process, real stdio, and a real SDK connection rather than a stub.

use std::path::Path;
use std::sync::Arc;

use mcp_chat_core::acp::AcpEvent;
use mcp_chat_core::acp::manager::AcpManager;
use mcp_chat_core::acp::session::{
    PermissionDecision, PermissionHandler, PermissionRequestInfo, SessionSpec,
};

/// Path to the mock agent built alongside the tests.
fn mock_agent() -> String {
    env!("CARGO_BIN_EXE_mock-acp-agent").to_owned()
}

fn no_permissions() -> PermissionHandler {
    Arc::new(|_: PermissionRequestInfo| Box::pin(async { PermissionDecision::Cancel }))
}

/// Open a session against the mock agent with the given scenario as argv[1].
async fn open_session(
    manager: &AcpManager,
    local_id: &str,
    cwd: &Path,
    scenario: &str,
) -> (
    Arc<mcp_chat_core::acp::AcpSession>,
    tokio::sync::mpsc::UnboundedReceiver<AcpEvent>,
) {
    open_session_with(manager, local_id, cwd, scenario, no_permissions()).await
}

/// Same as [`open_session`], but with an explicit permission decision.
async fn open_session_with(
    manager: &AcpManager,
    local_id: &str,
    cwd: &Path,
    scenario: &str,
    permissions: PermissionHandler,
) -> (
    Arc<mcp_chat_core::acp::AcpSession>,
    tokio::sync::mpsc::UnboundedReceiver<AcpEvent>,
) {
    // A `+close` suffix makes the mock advertise `session/close` support.
    let (scenario, argv) = match scenario.strip_suffix("+close") {
        Some(scenario) => (scenario, vec![scenario.to_owned(), "close".to_owned()]),
        None => (scenario, vec![scenario.to_owned()]),
    };
    let command = mock_agent();
    let active = manager
        .open(SessionSpec {
            local_id: local_id.to_owned(),
            command: &command,
            args: &argv,
            env: &[],
            cwd,
            permissions,
        })
        .await
        .unwrap_or_else(|error| panic!("open failed: {}", error.user_message()));
    (active.handle, active.events)
}

#[tokio::test]
async fn multi_turn_session_reuses_one_process() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let (handle, mut events) = open_session(&manager, "child-1", &cwd, "echo").await;

    assert!(!handle.acp_session_id.is_empty());
    assert_eq!(manager.count().await, 1);

    handle.prompt("first question").unwrap();
    let first = collect_turn(&mut events).await;
    assert!(first.contains("turn 1 answer: first question"), "{first}");

    // The same session answers again: the process was not restarted.
    handle.prompt("second question").unwrap();
    let second = collect_turn(&mut events).await;
    assert!(
        second.contains("turn 2 answer: second question"),
        "{second}"
    );

    manager.close("child-1").await.unwrap();
    assert_eq!(manager.count().await, 0);
}

#[tokio::test]
async fn tool_call_update_becomes_a_tool_event() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let (handle, mut events) = open_session(&manager, "child-tool", &cwd, "tool").await;

    handle.prompt("inspect").unwrap();
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
            Ok(None) => panic!("event channel closed early; transcript so far: {transcript:?}"),
            Err(_) => panic!("timed out waiting for the tool turn"),
        }
    }
    assert!(saw_tool, "no tool call event");
    assert!(
        transcript.contains("Checking the file first."),
        "{transcript}"
    );
    manager.close("child-tool").await.unwrap();
}

#[tokio::test]
async fn missing_binary_is_reported_as_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let args: Vec<String> = Vec::new();
    let error = manager
        .open(SessionSpec {
            local_id: "child-missing".into(),
            command: "jarvis-missing-acp-agent",
            args: &args,
            env: &[],
            cwd: &cwd,
            permissions: no_permissions(),
        })
        .await
        .err()
        .expect("missing binary must fail");
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
    let (handle, mut events) = open_session(&manager, "child-crash", &cwd, "crash").await;

    handle.prompt("crash please").unwrap();
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
    // The slot must be reclaimed once the pump notices the process is gone.
    for _ in 0..100 {
        if manager.count().await == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        manager.count().await,
        0,
        "a crashed session must not keep its slot"
    );
    manager.close("child-crash").await.ok();
}

/// Approve everything, recording each request the agent made.
#[tokio::test]
async fn approving_a_permission_selects_allow_once_and_never_allow_always() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let approve: PermissionHandler = Arc::new(move |info: PermissionRequestInfo| {
        recorder.lock().unwrap().push(info.clone());
        Box::pin(async { PermissionDecision::Approve })
    });

    let (handle, mut events) =
        open_session_with(&manager, "child-permit", &cwd, "permission", approve).await;
    handle.prompt("run the tests").unwrap();
    let report = collect_turn(&mut events).await;

    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "expected one permission request");
    assert_eq!(
        requests[0].title, "Run tests",
        "agent-authored title is kept"
    );
    assert_eq!(requests[0].kind.as_deref(), Some("execute"));
    assert!(
        requests[0]
            .options
            .iter()
            .any(|(_, kind)| kind.as_deref() == Some("allow_always")),
        "the mock offers allow_always and it must still be visible to the UI"
    );

    // The agent echoes what the client actually selected on the wire.
    assert!(
        report.contains("permission=selected option=allow-once"),
        "approve must select the allow_once option, got: {report}"
    );
    manager.close("child-permit").await.unwrap();
}

#[tokio::test]
async fn rejecting_a_permission_selects_reject_once() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let reject: PermissionHandler =
        Arc::new(|_: PermissionRequestInfo| Box::pin(async { PermissionDecision::Reject }));
    let (handle, mut events) =
        open_session_with(&manager, "child-reject", &cwd, "permission", reject).await;
    handle.prompt("run the tests").unwrap();
    let report = collect_turn(&mut events).await;
    assert!(
        report.contains("permission=selected option=reject-once"),
        "reject must select the reject_once option, got: {report}"
    );
    manager.close("child-reject").await.unwrap();
}

#[tokio::test]
async fn cancelling_a_permission_answers_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let (handle, mut events) = open_session(&manager, "child-cancel", &cwd, "permission").await;
    handle.prompt("run the tests").unwrap();
    let report = collect_turn(&mut events).await;
    assert!(
        report.contains("permission=cancelled"),
        "cancel must answer the cancelled outcome, got: {report}"
    );
    manager.close("child-cancel").await.unwrap();
}

#[tokio::test]
async fn closing_a_session_does_not_report_a_failed_turn() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let (handle, mut events) = open_session(&manager, "child-quiet", &cwd, "echo").await;
    handle.prompt("hello").unwrap();
    let _ = collect_turn(&mut events).await;

    manager.close("child-quiet").await.unwrap();
    // A normal close must produce no spurious failure event.
    let stray = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv()).await;
    match stray {
        Ok(Some(AcpEvent::TurnFailed { message })) => {
            panic!("normal close reported a failure: {message}")
        }
        Ok(Some(other)) => panic!("unexpected event after close: {other:?}"),
        Ok(None) | Err(_) => {}
    }
    assert_eq!(manager.count().await, 0);
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
            Ok(None) => panic!("event channel closed before the turn ended; text so far: {text:?}"),
            Err(_) => panic!("timed out waiting for the turn; got so far: {text}"),
        }
    }
    text
}
