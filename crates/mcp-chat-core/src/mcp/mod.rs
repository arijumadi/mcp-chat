pub mod client;
pub mod legacy;
pub mod manager;
pub mod tools;
pub mod types;

pub use client::{ClientConfig, McpClient, McpConnection, MrtrRetry, ProgressEvent};
pub use manager::{ConnectionReport, McpManager};
pub use tools::{McpTool, ToolRegistry};
pub use types::{
    CallToolResult, McpError, ServerCapabilities, ServerInfo, ToolInfo, TransportKind,
};
