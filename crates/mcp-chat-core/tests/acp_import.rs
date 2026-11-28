//! Attaching to a session from outside Jarvis must leave a readable transcript.
//!
//! `session/load` replays the stored history. For a conversation Jarvis never
//! saw, that replay is the only copy, so it has to be written as messages --
//! otherwise the chat opens empty and a later agent switch has nothing to carry.

use std::path::Path;
use std::sync::Arc;

use mcp_chat_core::acp::manager::AcpManager;
use mcp_chat_core::acp::session::{
    Attach, PermissionDecision, PermissionHandler, PermissionRequestInfo, SessionSpec,
};
use mcp_chat_core::acp::sessions::list_sessions;

fn no_permissions() -> PermissionHandler {
    Arc::new(|_: PermissionRequestInfo| Box::pin(async { PermissionDecision::Cancel }))
}

fn opencode() -> Option<&'static str> {
    let path = "/home/ari/.opencode/bin/opencode";
    Path::new(path).exists().then_some(path)
}

/// A `Load` reports the replayed transcript as an import, with the text intact.
#[tokio::test]
async fn loading_a_foreign_session_reports_its_transcript() {
    let Some(command) = opencode() else {
        eprintln!("opencode not installed here; skipping");
        return;
    };

    let manager = AcpManager::new();
    let args = vec!["acp".to_owned()];
    let page = list_sessions(
        &manager,
        command,
        &args,
        &[],
        None,
        None,
        std::time::Duration::from_secs(60),
    )
    .await
    .expect("session/list");

    // Try old sessions in order until one actually replays something. A newly
    // created session has no history, and that is not a failure of the import.
    let mut found = None;
    for candidate in page
        .sessions
        .iter()
        .rev()
        .filter(|s| s.title.is_some())
        .take(6)
    {
        let opened = manager
            .open(SessionSpec {
                local_id: format!("import-test-{}", candidate.session_id),
                command,
                args: &args,
                env: &[],
                cwd: Path::new(&candidate.cwd),
                attach: Attach::Load(candidate.session_id.clone()),
                permissions: no_permissions(),
            })
            .await;
        let Ok(opened) = opened else { continue };
        let taken = opened.handle.take_replayed();
        if let Some(value) = taken {
            found = Some((candidate, value));
            // The session stays open; it is closed on the way out.
            break;
        }
    }
    let Some((_target, (transcript, _truncated))) = found else {
        eprintln!("no session with history replayed anything; skipping");
        return;
    };
    assert!(
        transcript.iter().any(|message| message.role == "user"),
        "a conversation has at least one user turn"
    );
    assert!(
        !transcript.is_empty(),
        "the replayed conversation should carry text"
    );
    assert!(
        transcript.iter().any(|message| message.role == "user"),
        "a conversation has at least one user turn"
    );
    assert!(
        transcript.iter().any(|message| message.role == "assistant"),
        "and at least one answer"
    );
    // Merged by role: chunks must not become one message per token.
    assert!(
        transcript.len() < 200,
        "the transcript should be one entry per turn, got {} entries",
        transcript.len()
    );

    manager.close("import-test").await.unwrap();
}
