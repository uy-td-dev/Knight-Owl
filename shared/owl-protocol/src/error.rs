//! Top-level protocol error enum.

use thiserror::Error;

#[derive(Debug, Clone, Error, serde::Serialize, serde::Deserialize)]
pub enum ProtocolError {
    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("completion error: {0}")]
    Completion(String),

    #[error("memory error: {0}")]
    Memory(String),

    #[error("mcp error: {0}")]
    Mcp(String),

    #[error("unknown error: {0}")]
    Unknown(String),
}
