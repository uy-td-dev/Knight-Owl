//! Code-context retrieval abstraction injected into the reasoning loop.
//!
//! Implementations live in the app layer (owl-cli, owl-desktop) where concrete
//! store dependencies are wired — owl-brain never imports owl-vault directly.

use async_trait::async_trait;

use crate::BrainError;

/// Provides relevant code context for a given user prompt.
///
/// Injected into [`crate::ReasoningLoop`] via `with_context`. When present,
/// the loop prepends retrieved snippets to every planning prompt so the LLM
/// can reason over real code rather than hallucinating structure.
#[async_trait]
pub trait ContextProvider: Send + Sync {
    /// Return a list of context strings relevant to `prompt`.
    ///
    /// Each string is a self-contained snippet (e.g. one code node with its
    /// file path, kind, and preview). The loop joins them with `---` separators
    /// inside a `<code_context>` block.
    async fn retrieve(&self, prompt: &str) -> Result<Vec<String>, BrainError>;
}
