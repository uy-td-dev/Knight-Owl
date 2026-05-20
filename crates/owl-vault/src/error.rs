//! Error types for owl-vault.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("surrealdb error: {0}")]
    Surreal(String),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl From<surrealdb::Error> for VaultError {
    fn from(e: surrealdb::Error) -> Self {
        VaultError::Surreal(e.to_string())
    }
}
