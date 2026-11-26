use agent_client_protocol::{AcpAgent, AcpAgentConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let script = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "spikes/acp-phase0/mock_agent.mjs".to_owned());
    let agent = AcpAgent::new(AcpAgentConfig::new("node").arg(script));
    let (first, second) = acp_phase0::run_multi_turn(agent).await?;
    println!("turn1={first}");
    println!("turn2={second}");
    Ok(())
}
