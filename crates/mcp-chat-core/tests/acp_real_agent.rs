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
    PermissionDecision, PermissionHandler, PermissionRequestInfo, SessionSpec, open,
};

fn agent_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("AGY_BIN") {
        return Some(PathBuf::from(path));
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["antigravity", "antigravity.exe", "agy", "agy.exe"] {
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
            resume: None,
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
