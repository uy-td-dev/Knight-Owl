//! Error types for owl-mcp.

use thiserror::Error;

use owl_protocol::mcp::McpError;

#[derive(Debug, Error)]
pub enum McpClientError {
    #[error("transport error: {0}")]
    Transport(#[from] McpError),

    #[error("not connected")]
    NotConnected,

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
