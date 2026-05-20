//! Error types for owl-cortex.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CortexError {
    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),

    #[error("parse error in {path}: {detail}")]
    Parse { path: String, detail: String },

    #[error("vault error: {0}")]
    Vault(#[from] owl_vault::VaultError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("lsp error: {0}")]
    Lsp(String),
}
