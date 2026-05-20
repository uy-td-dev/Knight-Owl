//! Memory types and trait shared between owl-brain (loop) and owl-vault (backend).
//!
//! Lives in owl-protocol so vault can implement [`MemoryStore`] without
//! importing owl-brain, preserving the R-13 dependency boundary.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A single turn in the agent's conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Role of the speaker: `"user"`, `"assistant"`, or `"tool"`.
    pub role: String,
    /// Text content of the entry.
    pub content: String,
}

/// Errors returned by [`MemoryStore`] operations.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum MemoryError {
    #[error("memory backend error: {0}")]
    Backend(String),
}

/// Pluggable memory backend for the reasoning loop.
///
/// Defined in owl-protocol so both owl-brain (loop) and owl-vault (SurrealDB
/// impl) can depend on it without creating a circular dependency.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Append an entry to the store.
    async fn push(&self, entry: MemoryEntry) -> Result<(), MemoryError>;

    /// Retrieve the `limit` most-recent entries, oldest first.
    async fn recent(&self, limit: usize) -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Retrieve entries most relevant to `prompt`, oldest first.
    ///
    /// Backends with embedding support blend semantic similarity with recency.
    /// The default implementation falls back to [`Self::recent`].
    async fn recall(
        &self,
        _prompt: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.recent(limit).await
    }

    /// Clear all stored entries.
    async fn clear(&self) -> Result<(), MemoryError>;
}
