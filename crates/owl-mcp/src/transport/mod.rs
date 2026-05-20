//! Transport abstraction for MCP communication.

pub mod stdio;

use async_trait::async_trait;
use owl_protocol::mcp::{McpError, McpMessage};

/// Transport kind — selected at startup via config, never conditionally inside client.rs.
#[derive(Debug, Clone)]
pub enum TransportKind {
    Stdio,
}

/// Abstraction over the physical MCP transport layer.
///
/// Concrete impls: [`stdio::StdioTransport`].
/// Retry and reconnect logic lives in `client.rs`, not here.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a message and return the response.
    async fn send(&self, msg: McpMessage) -> Result<McpMessage, McpError>;

    /// Close the transport connection gracefully.
    async fn close(&self) -> Result<(), McpError>;
}
