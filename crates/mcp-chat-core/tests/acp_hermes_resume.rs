// Prove the real Hermes path: a session with history, resumed by Jarvis code,
// must not duplicate the transcript.
//
// This drives the actual mcp-chat-core ACP client, not a hand-written probe, so
// what is verified is the code Jarvis ships.
use std::path::Path;
use std::sync::Arc;

use mcp_chat_core::acp::AcpEvent;
use mcp_chat_core::acp::manager::AcpManager;
use mcp_chat_core::acp::session::{
    PermissionDecision, PermissionHandler, PermissionRequestInfo, SessionSpec,
};

fn no_permissions() -> PermissionHandler {
    Arc::new(|_: PermissionRequestInfo| Box::pin(async { PermissionDecision::Cancel }))
}

fn hermes() -> Option<String> {
    let path = "/home/ari/.hermes/hermes-agent/.hermes/bin/hermes";
    Path::new(path).exists().then(|| path.to_owned())
}

/// Collect events until the turn ends or `ms` elapses.
async fn collect(
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
            Ok(Some(event)) => {
                let ended = matches!(
                    event,
                    AcpEvent::TurnEnded { .. } | AcpEvent::TurnFailed { .. }
                );
                got.push(event);
                if ended {
                    break;
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    got
}

fn texts(events: &[AcpEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            AcpEvent::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn hermes_does_not_duplicate_history_on_resume() {
    let Some(command) = hermes() else {
        eprintln!("hermes is not installed here; skipping");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let args = vec!["acp".to_owned()];

    // 1. A fresh session that learns a codeword.
    let first = manager
        .open(SessionSpec {
            local_id: "real-first".into(),
            command: &command,
            args: &args,
            env: &[],
            cwd: &cwd,
            resume: None,
            permissions: no_permissions(),
        })
        .await
        .expect("hermes should open");

    first
        .handle
        .prompt("Remember the codeword PELICAN-7788. Reply with just OK.")
        .unwrap();
    let mut events = first.events;
    let turn = collect(&mut events, 120_000).await;
    assert!(
        texts(&turn).contains("OK") || !texts(&turn).is_empty(),
        "first turn should answer, got {:?}",
        texts(&turn)
    );
    let session_id = first.handle.acp_session_id.clone();
    manager.close("real-first").await.unwrap();
    drop(events);

    // 2. Reopen with that id, the way Jarvis does after a restart.
    let second = manager
        .open(SessionSpec {
            local_id: "real-second".into(),
            command: &command,
            args: &args,
            env: &[],
            cwd: &cwd,
            resume: Some(session_id.clone()),
            permissions: no_permissions(),
        })
        .await
        .expect("resume should open");

    assert!(
        second.handle.resumed(),
        "hermes should accept the stored session id"
    );
    let mut events = second.events;

    // Whatever the reopen produced before the first prompt is the replay.
    let settled = collect(&mut events, 3_000).await;
    let replay_text = texts(&settled);
    assert!(
        replay_text.is_empty(),
        "replayed history must not surface as new text, got {replay_text:?}"
    );

    // 3. The session must still know its history.
    second
        .handle
        .prompt("What was the codeword? One word.")
        .unwrap();
    let answer = collect(&mut events, 120_000).await;
    let text = texts(&answer);
    assert!(
        text.contains("PELICAN"),
        "the resumed session should still know its history, got {text:?}"
    );

    // And the answer must appear exactly once, not once per replay copy.
    let occurrences = text.matches("PELICAN-7788").count();
    assert!(
        occurrences <= 1,
        "the codeword appears {occurrences} times; history was duplicated: {text:?}"
    );

    manager.close("real-second").await.unwrap();
}
