//! Memory abstraction for the reasoning loop.
//!
//! `MemoryStore` and `MemoryEntry` live in `owl-protocol` so the SurrealDB
//! backend in `owl-vault` can implement the trait without importing owl-brain.
//!
//! This module re-exports them and provides the in-process fallback.

use async_trait::async_trait;

pub use owl_protocol::memory::{MemoryEntry, MemoryError, MemoryStore};

use crate::BrainError;

/// Simple in-memory implementation for development and tests.
pub struct InMemoryStore {
    entries: tokio::sync::RwLock<Vec<MemoryEntry>>,
}

impl InMemoryStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self { entries: tokio::sync::RwLock::new(Vec::new()) }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn push(&self, entry: MemoryEntry) -> Result<(), MemoryError> {
        self.entries.write().await.push(entry);
        Ok(())
    }

    async fn recent(&self, limit: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
        let entries = self.entries.read().await;
        let start = entries.len().saturating_sub(limit);
        Ok(entries[start..].to_vec())
    }

    async fn clear(&self) -> Result<(), MemoryError> {
        self.entries.write().await.clear();
        Ok(())
    }
}

impl From<MemoryError> for BrainError {
    fn from(e: MemoryError) -> Self {
        BrainError::Memory(e.to_string())
    }
}
