//! Error types for owl-brain.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrainError {
    #[error("completion failed: {0}")]
    Completion(String),

    #[error("tool dispatch failed: {0}")]
    ToolDispatch(String),

    #[error("memory unavailable: {0}")]
    Memory(String),

    #[error("max reasoning steps ({0}) exceeded")]
    MaxStepsExceeded(usize),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("context retrieval failed: {0}")]
    ContextRetrieval(String),

    #[error("agent configuration error: {0}")]
    Config(String),
}
