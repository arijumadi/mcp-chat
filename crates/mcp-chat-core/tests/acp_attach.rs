//! Opening a session that Jarvis did not create.
//!
//! `Attach::Load` is the only path that can continue a conversation made in an
//! agent's own CLI. It must use the session's own `cwd`, must fail rather than
//! silently start a new session, and must still hand back the old context.

use std::path::Path;
use std::sync::Arc;

use mcp_chat_core::acp::AcpEvent;
use mcp_chat_core::acp::manager::AcpManager;
use mcp_chat_core::acp::session::{
    Attach, PermissionDecision, PermissionHandler, PermissionRequestInfo, SessionSpec,
};
use mcp_chat_core::acp::sessions::list_sessions;

fn no_permissions() -> PermissionHandler {
    Arc::new(|_: PermissionRequestInfo| Box::pin(async { PermissionDecision::Cancel }))
}

fn agent_command() -> Option<(&'static str, Vec<String>)> {
    let candidates = [
        ("/home/ari/.opencode/bin/opencode", "opencode"),
        ("/home/ari/.local/bin/agy", "antigravity"),
    ];
    for (path, name) in candidates {
        if Path::new(path).exists() {
            return Some((path, vec!["acp".to_owned(), name.to_owned()]));
        }
    }
    None
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

#[tokio::test]
async fn a_session_from_outside_jarvis_opens_with_its_own_cwd() {
    let Some((command, mut argv)) = agent_command() else {
        eprintln!("no ACP agent installed here; skipping");
        return;
    };
    // The first argument names the agent for the `candidates` lookup only.
    argv.truncate(1);

    let manager = AcpManager::new();
    let page = list_sessions(
        &manager,
        command,
        &argv,
        &[],
        None,
        None,
        std::time::Duration::from_secs(60),
    )
    .await
    .expect("session/list should answer");

    if page.sessions.is_empty() {
        eprintln!("this agent lists no sessions; skipping");
        return;
    }

    // Prefer the oldest entry: a session created moments ago can be empty or
    // already gone, while the tail of the list is established history -- the
    // kind of session this feature exists for.
    let target = page
        .sessions
        .iter()
        .rev()
        .find(|session| session.title.is_some())
        .unwrap_or(&page.sessions[0]);

    println!(
        "attaching to {} ({:?}) in {}",
        target.session_id, target.title, target.cwd
    );

    let manager = AcpManager::new();
    let created = manager
        .open(SessionSpec {
            local_id: "attach-test".into(),
            command,
            args: &argv,
            env: &[],
            // The session's own directory, never the chat's workspace.
            cwd: Path::new(&target.cwd),
            attach: Attach::Load(target.session_id.clone()),
            permissions: no_permissions(),
        })
        .await
        .expect("load should open the existing session");

    assert!(
        created.handle.resumed(),
        "the agent must confirm it loaded the session, not start a new one"
    );
    assert_eq!(
        created.handle.acp_session_id, target.session_id,
        "the id must be the one we asked for"
    );

    let mut events = created.events;
    let replay = collect(&mut events, 3_000).await;
    assert!(
        !replay.is_empty(),
        "loading should replay the stored history"
    );

    manager.close("attach-test").await.unwrap();
}

/// An id that does not exist must fail. Falling back to a new session here
/// would silently attach the chat to an empty conversation.
#[tokio::test]
async fn loading_an_unknown_session_fails_instead_of_starting_a_new_one() {
    let Some((command, mut argv)) = agent_command() else {
        eprintln!("no ACP agent installed here; skipping");
        return;
    };
    argv.truncate(1);

    let manager = AcpManager::new();
    let result = manager
        .open(SessionSpec {
            local_id: "attach-missing".into(),
            command,
            args: &argv,
            env: &[],
            cwd: Path::new("/home/ari"),
            attach: Attach::Load("definitely-not-a-real-session-id".into()),
            permissions: no_permissions(),
        })
        .await;

    assert!(
        result.is_err(),
        "an unknown session id must be an error, not a fresh session"
    );
}
