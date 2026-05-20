//! Error types for owl-tower.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TowerError {
    #[error("provider configuration error: {0}")]
    Config(String),

    #[error("completion request failed: {0}")]
    Completion(String),

    #[error("unsupported provider: {0}")]
    UnsupportedProvider(String),
}
