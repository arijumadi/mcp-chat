use mcp_mock::{mcp, provider};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let mut modern_port = 8787;
    let mut provider_port = 8788;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--modern-port" => {
                modern_port = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(8787);
                i += 2;
            }
            "--provider-port" => {
                provider_port = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(8788);
                i += 2;
            }
            _ => i += 1,
        }
    }

    let modern = mcp::start_at(mcp::McpMode::Modern, modern_port).await;
    let legacy = mcp::start_at(mcp::McpMode::Legacy, modern_port + 2).await;
    let sse = mcp::start_at(mcp::McpMode::Sse, modern_port + 3).await;
    let provider = provider::start_at(provider_port).await;

    println!("mcp-chat mock servers");
    println!("  MCP 2026-07-28 : {}", modern.url);
    println!("  MCP 2025-11-25 : {}", legacy.url);
    println!("  MCP HTTP+SSE   : {}", sse.url);
    println!("  OpenAI-compat  : {}", provider.url);
    println!("  Models         : mock-text, mock-tool, mock-parallel, mock-cut, mock-overflow");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();
}
