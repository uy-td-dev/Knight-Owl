//! The Cortex — L1 Syntax + L2 Logic layers of the nervous system.
//!
//! Drives tree-sitter AST parsing and LSP-assisted cross-file resolution,
//! populating `code_node` / `file` tables in the hybrid store.
//!
//! Entry point: [`Ingestor::on_file_changed`].
//!
//! `owl-cortex` depends on `owl-protocol`, `owl-vault`, `tree-sitter` only.

#![forbid(unsafe_code)]

pub mod ast;
pub mod config;
pub mod error;
pub mod ingest;
pub mod lsp;

pub use config::Config;
pub use error::CortexError;
pub use ingest::Ingestor;
pub use lsp::LspClient;

/// Derive the SurrealDB `file` record id for a relative path.
///
/// Mirrors the sanitisation in `owl-vault::surreal::sanitize_id`.
pub fn file_id(rel_path: &str) -> String {
    rel_path
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect()
}
