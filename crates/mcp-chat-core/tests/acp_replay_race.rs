//! Reopen while a turn is still running.
//!
//! The desktop app can switch away from a chat and back while the agent is
//! still working. The reopen path resumes, and Hermes replays history on resume.
//! If the replay is not drained before the next prompt, its chunks interleave
//! with the live turn and the answer arrives twice.

use std::path::Path;
use std::sync::Arc;

use mcp_chat_core::acp::AcpEvent;
use mcp_chat_core::acp::manager::AcpManager;
use mcp_chat_core::acp::session::{
    Attach, PermissionDecision, PermissionHandler, PermissionRequestInfo, SessionSpec,
};

fn no_permissions() -> PermissionHandler {
    Arc::new(|_: PermissionRequestInfo| Box::pin(async { PermissionDecision::Cancel }))
}

fn hermes() -> Option<String> {
    let path = "/home/ari/.hermes/hermes-agent/.hermes/bin/hermes";
    Path::new(path).exists().then(|| path.to_owned())
}

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

/// A reopen must settle the replay before it accepts a prompt, otherwise the
/// replay lands inside the answer. This drives open() then immediately prompt()
/// with no pause, which is the tightest race the real app can produce.
#[tokio::test]
async fn a_reopen_then_immediate_prompt_does_not_mix_the_replay() {
    let Some(command) = hermes() else {
        eprintln!("hermes is not installed here; skipping");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let manager = AcpManager::new();
    let args = vec!["acp".to_owned()];

    // Seed a session with a distinctive first turn.
    let first = manager
        .open(SessionSpec {
            local_id: "race-first".into(),
            command: &command,
            args: &args,
            env: &[],
            cwd: &cwd,
            attach: Attach::New,
            permissions: no_permissions(),
        })
        .await
        .expect("hermes should open");
    first
        .handle
        .prompt("Say exactly: SEED-1234. Nothing else.")
        .unwrap();
    let mut first_events = first.events;
    let _ = collect(&mut first_events, 120_000).await;
    let session_id = first.handle.acp_session_id.clone();
    manager.close("race-first").await.unwrap();
    drop(first_events);

    // Reopen and prompt with no delay at all.
    let second = manager
        .open(SessionSpec {
            local_id: "race-second".into(),
            command: &command,
            args: &args,
            env: &[],
            cwd: &cwd,
            attach: Attach::Resume(session_id),
            permissions: no_permissions(),
        })
        .await
        .expect("resume should open");
    second
        .handle
        .prompt("What word did you say before? One word.")
        .unwrap();

    let mut events = second.events;
    let turn = collect(&mut events, 120_000).await;
    let text = texts(&turn);

    // SEED-1234 legitimately appears once, as the answer. More than that means
    // the replayed copy of the first turn leaked into this one.
    let seeds = text.matches("SEED-1234").count();
    assert!(
        seeds <= 1,
        "the replayed turn leaked into the live answer ({seeds} copies): {text:?}"
    );

    manager.close("race-second").await.unwrap();
}
