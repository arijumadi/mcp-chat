//! Mock ACP agent used by integration tests. Speaks ACP v1 over stdio.

use std::io::Write;

fn main() {
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "echo".to_owned());
    mcp_chat_core::acp::mock_agent::run(&scenario);
    let _ = std::io::stdout().flush();
}
