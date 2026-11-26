//! Throwaway Phase 0 spike for the ACP client design.
//!
//! It deliberately lives outside the main workspace. The goal is to prove the
//! SDK's persistent v1 session API and close path against a stdio mock agent.
//! Do not merge this crate into the application.

use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{CloseSessionRequest, InitializeRequest};
use agent_client_protocol::v1::SessionSetup;
use agent_client_protocol::{AcpAgent, Client};

/// Run one ACP session with two prompts, then close it.
///
/// The same `ActiveSession` is reused for both prompts. The returned strings
/// are the text chunks received for each prompt.
pub async fn run_multi_turn(agent: AcpAgent) -> anyhow::Result<(String, String)> {
    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let emit = captured.clone();

    Client
        .v1()
        .name("jarvis-acp-phase0")
        .connect_with(agent, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            cx.build_session_cwd()?
                .block_task()
                .run_until(async move |SessionSetup { mut session, .. }| {
                    session.send_prompt("phase0 turn one")?;
                    let first = session.read_to_string().await?;
                    emit.lock().expect("phase0 capture").push(first);

                    session.send_prompt("phase0 turn two")?;
                    let second = session.read_to_string().await?;
                    emit.lock().expect("phase0 capture").push(second);

                    let _ = session
                        .connection()
                        .send_request(CloseSessionRequest::new(session.session_id().clone()))
                        .block_task()
                        .await?;
                    Ok(())
                })
                .await
        })
        .await?;

    let mut values = captured.lock().expect("phase0 capture").clone();
    anyhow::ensure!(
        values.len() == 2,
        "expected two prompt responses, got {}",
        values.len()
    );
    Ok((values.remove(0), values.remove(0)))
}
