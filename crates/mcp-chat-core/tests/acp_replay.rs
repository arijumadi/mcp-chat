//! A resumed session must not duplicate the transcript.
//!
//! Hermes replays history on `session/resume` (and on `session/load`) by
//! streaming `session/update` notifications *before* the response. Those chunks
//! look exactly like live output. A client that appends them shows the old
//! conversation a second time.
//!
//! Verified against hermes-agent 0.21.5: resuming a session that had one turn
//! produced `user_message_chunk` + `agent_message_chunk` carrying the old text.

use std::path::Path;
use std::sync::Arc;

use mcp_chat_core::acp::AcpEvent;
use mcp_chat_core::acp::manager::AcpManager;
use mcp_chat_core::acp::session::{
    Attach, PermissionDecision, PermissionHandler, PermissionRequestInfo, SessionSpec,
};

fn mock_agent() -> String {
    env!("CARGO_BIN_EXE_mock-acp-agent").to_owned()
}

fn no_permissions() -> PermissionHandler {
    Arc::new(|_: PermissionRequestInfo| Box::pin(async { PermissionDecision::Cancel }))
}

async fn open(
    manager: &AcpManager,
    local_id: &str,
    cwd: &Path,
    scenario: &str,
    resume: Option<&str>,
) -> (
    Arc<mcp_chat_core::acp::AcpSession>,
    tokio::sync::mpsc::UnboundedReceiver<AcpEvent>,
) {
    let argv = vec![scenario.to_owned()];
    let command = mock_agent();
    let active = manager
        .open(SessionSpec {
            local_id: local_id.to_owned(),
            command: &command,
            args: &argv,
            env: &[],
            cwd,
            attach: match resume {
                Some(id) => Attach::Resume(id.to_owned()),
                None => Attach::New,
            },
            permissions: no_permissions(),
        })
        .await
        .expect("session should open");
    (active.handle, active.events)
}

/// Drain whatever the pump emitted, waiting at most `ms` for the first event.
async fn drain(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<AcpEvent>,
    ms: u64,
) -> Vec<AcpEvent> {
    let mut got = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, events.recv()).await {
            Ok(Some(event)) => got.push(event),
            Ok(None) | Err(_) => break,
        }
    }
    got
}

#[tokio::test]
async fn a_replayed_resume_reaches_the_caller_as_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();

    let (_handle, mut events) =
        open(&manager, "child-replay", &cwd, "replay", Some("old-id")).await;

    // The replay arrives before the resume response, so it is already queued.
    let seen = drain(&mut events, 1_500).await;
    let texts: Vec<String> = seen
        .iter()
        .filter_map(|event| match event {
            AcpEvent::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect();

    assert!(
        texts.is_empty(),
        "replayed history must not arrive as new text, got {texts:?}"
    );

    let notices: Vec<String> = seen
        .iter()
        .filter_map(|event| match event {
            AcpEvent::HistoryReplayed { messages, .. } => Some(messages.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 1, "expected one replay notice, got {seen:?}");
    // The summary counts what was skipped. The text itself is deliberately not
    // carried: it is already in the transcript, and the point is that it was
    // dropped, not re-sent.
    assert!(
        notices[0].contains("2 messages"),
        "the notice should count what was skipped: {}",
        notices[0]
    );

    manager.close("child-replay").await.unwrap();
}

#[tokio::test]
async fn a_normal_resume_still_answers_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();

    let (handle, mut events) = open(&manager, "child-plain", &cwd, "echo", Some("old-id")).await;
    handle.prompt("still there?").unwrap();

    let mut texts = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(500), events.recv()).await {
            Ok(Some(AcpEvent::Text(text))) => texts.push_str(&text),
            Ok(Some(AcpEvent::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    assert!(texts.contains("still there?"), "got {texts:?}");
    manager.close("child-plain").await.unwrap();
}
