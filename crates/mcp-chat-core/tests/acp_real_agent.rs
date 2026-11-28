//! Probe against a real vendor ACP agent, when one is installed.
//!
//! Ignored by default: it needs the vendor binary and, for a full turn, a
//! signed-in account. Keeping it in the repository makes the check repeatable
//! instead of folklore in a commit message.
//!
//!     AGY_BIN=/path/to/antigravity cargo test -p mcp-chat-core \
//!         --test acp_real_agent -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use mcp_chat_core::acp::AcpEvent;
use mcp_chat_core::acp::session::{
    Attach, PermissionDecision, PermissionHandler, PermissionRequestInfo, SessionSpec, open,
};

fn agent_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("AGY_BIN") {
        return Some(PathBuf::from(path));
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        // The official installer exposes the binary as `agy`; the tarball's
        // inner name `antigravity` is checked too, for a manual unpack.
        for name in ["agy", "agy.exe", "antigravity", "antigravity.exe"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn no_permissions() -> PermissionHandler {
    Arc::new(|_: PermissionRequestInfo| Box::pin(async { PermissionDecision::Cancel }))
}

#[tokio::test]
#[ignore = "needs the Antigravity CLI"]
async fn a_real_antigravity_agent_reports_its_capabilities() {
    let Some(binary) = agent_binary() else {
        eprintln!("skipping: no antigravity binary found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let args = vec!["acp".to_owned()];
    let (events_tx, _events) = tokio::sync::mpsc::unbounded_channel::<AcpEvent>();

    let opened = open(
        SessionSpec {
            local_id: "agy".into(),
            command: binary.to_str().unwrap(),
            args: &args,
            env: &[],
            cwd: &cwd,
            attach: Attach::New,
            permissions: no_permissions(),
        },
        events_tx,
    )
    .await;

    match opened {
        Ok(session) => {
            eprintln!(
                "agent={:?} image={} embedded={}",
                session.agent_info, session.accepts_images, session.accepts_embedded_context
            );
            assert!(
                session.accepts_images,
                "Antigravity advertises image support"
            );
            assert!(
                session.accepts_embedded_context,
                "Antigravity advertises embedded context"
            );
            let _ = session.close();
        }
        Err(error) => {
            // On a clean machine the vendor refuses `session/new` until the
            // user signs in. That is the expected result; what matters is that
            // the message names the login instead of dumping a protocol error.
            let message = error.user_message();
            eprintln!("open failed as expected: {message}");
            assert!(
                message.contains("needs a login"),
                "the failure should tell the user to sign in: {message}"
            );
        }
    }
}

/// A full turn against a signed-in vendor agent.
///
/// Ignored because it needs an account. This is the check that proves the
/// client can actually complete a prompt end to end, not merely handshake.
#[tokio::test]
#[ignore = "needs the Antigravity CLI, signed in"]
async fn a_real_antigravity_agent_answers_a_prompt_and_remembers_it() {
    let Some(binary) = agent_binary() else {
        eprintln!("skipping: no antigravity binary found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let args = vec!["acp".to_owned()];
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel::<AcpEvent>();

    let session = match open(
        SessionSpec {
            local_id: "agy-turn".into(),
            command: binary.to_str().unwrap(),
            args: &args,
            env: &[],
            cwd: &cwd,
            attach: Attach::New,
            permissions: no_permissions(),
        },
        events_tx,
    )
    .await
    {
        Ok(session) => session,
        Err(error) => {
            eprintln!("skipping: {}", error.user_message());
            return;
        }
    };

    session
        .prompt("Reply with exactly: ACP-OK")
        .expect("prompt sent");
    let first = collect_turn(&mut events).await;
    assert!(!first.trim().is_empty(), "the agent must answer");
    eprintln!("first answer: {first}");
    assert!(first.contains("ACP-OK"), "unexpected answer: {first}");

    // Same session, new prompt: the agent keeps its own history, which is why
    // Jarvis sends only the newest user message.
    session
        .prompt("Repeat the exact token I asked for in one word.")
        .expect("second prompt sent");
    let second = collect_turn(&mut events).await;
    eprintln!("second answer: {second}");
    assert!(
        second.contains("ACP-OK"),
        "the agent should remember the first turn: {second}"
    );

    let _ = session.close();
}

/// Gather assistant text until the turn ends.
async fn collect_turn(events: &mut tokio::sync::mpsc::UnboundedReceiver<AcpEvent>) -> String {
    let mut text = String::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(300), events.recv()).await {
            Ok(Some(AcpEvent::Text(chunk))) => text.push_str(&chunk),
            Ok(Some(AcpEvent::TurnEnded { .. })) => break,
            Ok(Some(AcpEvent::TurnFailed { message })) => panic!("turn failed: {message}"),
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed before the turn ended; got: {text}"),
            Err(_) => panic!("timed out; got so far: {text}"),
        }
    }
    text
}
