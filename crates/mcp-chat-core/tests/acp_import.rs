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

    let target = page
        .sessions
        .iter()
        .rev()
        .find(|session| session.title.is_some());
    let Some(target) = target else {
        eprintln!("no titled sessions; skipping");
        return;
    };

    let opened = manager
        .open(SessionSpec {
            local_id: "import-test".into(),
            command,
            args: &args,
            env: &[],
            cwd: Path::new(&target.cwd),
            attach: Attach::Load(target.session_id.clone()),
            permissions: no_permissions(),
        })
        .await
        .expect("load should open");

    // The transcript is settled during `open` and left on the handle, because
    // it arrives before any event consumer exists. Reading it here is what the
    // attach path does.
    let taken = opened.handle.take_replayed();
    eprintln!(
        "target={:?} title={:?} replayed={:?}",
        target.session_id,
        target.title,
        taken.as_ref().map(|(t, _)| t.len())
    );
    let (transcript, _truncated) = taken.expect("a load must leave its transcript on the handle");
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
