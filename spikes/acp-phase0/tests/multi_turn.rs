use agent_client_protocol::{AcpAgent, AcpAgentConfig};

#[tokio::test]
async fn multi_turn_stdio_lifecycle() -> anyhow::Result<()> {
    let script = format!("{}/mock_agent.mjs", env!("CARGO_MANIFEST_DIR"));
    let agent = AcpAgent::new(AcpAgentConfig::new("node").arg(script));

    let (first, second) = acp_phase0::run_multi_turn(agent).await?;
    assert!(first.contains("turn 1"), "unexpected first turn: {first}");
    assert!(second.contains("turn 2"), "unexpected second turn: {second}");
    Ok(())
}

#[tokio::test]
async fn manual_byte_streams_multi_turn() -> anyhow::Result<()> {
    let script = format!("{}/mock_agent.mjs", env!("CARGO_MANIFEST_DIR"));
    let (first, second) = acp_phase0::run_multi_turn_manual(&script).await?;
    assert!(first.contains("turn 1"), "unexpected manual first turn: {first}");
    assert!(second.contains("turn 2"), "unexpected manual second turn: {second}");
    Ok(())
}
