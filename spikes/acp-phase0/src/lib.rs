//! Throwaway Phase 0 spike for the ACP client design.
//!
//! It deliberately lives outside the main workspace. The goal is to prove the
//! SDK's persistent v1 session API and close path against a stdio mock agent.
//! Do not merge this crate into the application.

use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::ByteStreams;
use agent_client_protocol::v1::schema::{CloseSessionRequest, InitializeRequest};
use agent_client_protocol::v1::SessionSetup;
use agent_client_protocol::{AcpAgent, Client, ConnectTo};
use std::process::Stdio;
use tokio::process::Command;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Run one ACP session with two prompts, then close it.
///
/// The same `ActiveSession` is reused for both prompts. The returned strings
/// are the text chunks received for each prompt.
pub async fn run_multi_turn(agent: AcpAgent) -> anyhow::Result<(String, String)> {
    run_with_transport(agent).await
}

/// Same lifecycle, but use our own process spawn and the SDK byte-stream
/// adapter. This is the alternative to the `process` feature.
pub async fn run_multi_turn_manual(script: &str) -> anyhow::Result<(String, String)> {
    let mut child = Command::new("node")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().expect("child stdout");
    let stdin = child.stdin.take().expect("child stdin");
    let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
    let result = run_with_transport(transport).await;
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

async fn run_with_transport<T>(transport: T) -> anyhow::Result<(String, String)>
where
    T: ConnectTo<agent_client_protocol::Client> + Send + 'static,
{
    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let emit = captured.clone();

    Client
        .v1()
        .name("jarvis-acp-phase0")
        .connect_with(transport, async move |cx| {
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
