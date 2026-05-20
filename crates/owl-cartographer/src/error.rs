//! Error types for owl-cartographer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CartographerError {
    #[error("extraction LLM call failed: {0}")]
    Extraction(String),

    #[error("vault error: {0}")]
    Vault(#[from] owl_vault::VaultError),

    #[error("graph error: {0}")]
    Graph(String),

    #[error("invalid LLM extraction response: {0}")]
    InvalidResponse(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
