//! Error types for owl-armory.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArmoryError {
    #[error("tool not found: {0}")]
    NotFound(String),

    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("execution failed: {0}")]
    Execution(String),

    #[error("path not allowed: {0}")]
    PathNotAllowed(String),
}
