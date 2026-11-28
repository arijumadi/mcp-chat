//! `session/list` against the agents actually installed here.
//!
//! Skipped when an agent is missing, so the test is safe on CI. The point is to
//! keep the capability gate honest: an agent without
//! `sessionCapabilities.list` must report `supported: false`, not an error and
//! not an empty page that looks like "you have no sessions".

use std::path::Path;

use mcp_chat_core::acp::manager::AcpManager;
use mcp_chat_core::acp::sessions::list_sessions;

fn args() -> Vec<String> {
    vec!["acp".to_owned()]
}

#[tokio::test]
async fn opencode_lists_sessions_and_reports_the_real_cwd() {
    let command = "/home/ari/.opencode/bin/opencode";
    if !Path::new(command).exists() {
        eprintln!("opencode not installed here; skipping");
        return;
    }

    let manager = AcpManager::new();
    let page = list_sessions(
        &manager,
        command,
        &args(),
        &[],
        Some(Path::new("/home/ari")),
        None,
        std::time::Duration::from_secs(60),
    )
    .await
    .expect("session/list should answer");

    assert!(page.supported, "opencode advertises session/list");
    assert!(
        !page.sessions.is_empty(),
        "opencode should have sessions from the CLI too"
    );

    // Every entry must carry the directory it belongs to, because a later load
    // has to use exactly that value.
    for session in &page.sessions {
        assert!(
            !session.cwd.is_empty(),
            "session {} has no cwd",
            session.session_id
        );
        assert!(
            session.cwd.starts_with('/'),
            "cwd should be absolute, got {:?}",
            session.cwd
        );
    }
}

#[tokio::test]
async fn antigravity_lists_sessions_from_outside_jarvis() {
    let command = "/home/ari/.local/bin/agy";
    if !Path::new(command).exists() {
        eprintln!("antigravity not installed here; skipping");
        return;
    }

    let manager = AcpManager::new();
    let page = list_sessions(
        &manager,
        command,
        &args(),
        &[],
        Some(Path::new("/home/ari")),
        None,
        std::time::Duration::from_secs(60),
    )
    .await
    .expect("session/list should answer");

    assert!(page.supported);
    assert!(!page.sessions.is_empty());
    // Titles are optional in the protocol; when present they are what makes the
    // picker usable.
    assert!(
        page.sessions.iter().any(|session| session.title.is_some()),
        "at least some sessions should carry a title"
    );
}

/// Hermes filters to sessions it created through ACP. Listing must still work;
/// an empty result is valid and must not be reported as unsupported.
#[tokio::test]
async fn hermes_reports_support_separately_from_an_empty_list() {
    let command = "/home/ari/.hermes/hermes-agent/.hermes/bin/hermes";
    if !Path::new(command).exists() {
        eprintln!("hermes not installed here; skipping");
        return;
    }

    let manager = AcpManager::new();
    let page = list_sessions(
        &manager,
        command,
        &args(),
        &[],
        Some(Path::new("/home/ari")),
        None,
        std::time::Duration::from_secs(60),
    )
    .await
    .expect("session/list should answer");

    assert!(page.supported, "hermes advertises sessionCapabilities.list");
    // Whether it has rows is environment-specific; support is what matters.
}

/// An agent that does not advertise `sessionCapabilities.list` must report
/// `supported: false`. Returning an empty page instead would be
/// indistinguishable from "you have no sessions", which is a different thing.
#[tokio::test]
async fn an_agent_without_list_support_says_so() {
    let command = env!("CARGO_BIN_EXE_mock-acp-agent");

    let manager = AcpManager::new();
    // A directory that exists on every platform: the mock is spawned there.
    let dir = tempfile::tempdir().unwrap();
    let page = list_sessions(
        &manager,
        command,
        &[],
        &[],
        Some(dir.path()),
        None,
        std::time::Duration::from_secs(30),
    )
    .await
    .expect("the mock should still open");

    assert!(
        !page.supported,
        "the mock does not advertise sessionCapabilities.list"
    );
    assert!(page.sessions.is_empty());
}

/// Listing must not consume one of the four live-session slots.
///
/// `session/list` needs an initialized connection, and the only way in through
/// this SDK creates a session. That session has to be closed again: otherwise a
/// few visits to the picker leave no room to attach anything, and the failure
/// looks like "At most 4 ACP sessions can run at once" with nothing open.
#[tokio::test]
async fn listing_does_not_hold_a_session_slot() {
    let command = "/home/ari/.opencode/bin/opencode";
    if !Path::new(command).exists() {
        eprintln!("opencode not installed here; skipping");
        return;
    }

    let manager = AcpManager::new();
    for round in 0..6 {
        let page = list_sessions(
            &manager,
            command,
            &args(),
            &[],
            Some(Path::new("/home/ari")),
            None,
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap_or_else(|error| panic!("round {round} failed: {error}"));
        assert!(page.supported);
        assert_eq!(
            manager.count().await,
            0,
            "round {round} left a session registered"
        );
    }
}
