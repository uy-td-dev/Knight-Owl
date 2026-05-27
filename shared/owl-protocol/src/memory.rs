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

/// One hit from cross-session memory FTS — emitted by
/// `MemoryStore::search_session_memory` so callers can decide
/// whether to inject past conversations into the current context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecallHit {
    /// Session the matched entry belongs to.
    pub session_id: String,
    /// Role at the time it was recorded.
    pub role: String,
    /// Entry content.
    pub content: String,
    /// Backend-reported relevance.  Higher = better; semantics depend on
    /// the backend (BM25 score for SurrealDB, 0.0 for substring fallback).
    pub score: f32,
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

    /// Full-text search across EVERY session's conversation history.
    ///
    /// Use case: "did we discuss X in any past chat?" — the agent (or UI)
    /// calls this to surface relevant turns from sessions other than the
    /// one it's currently in.
    ///
    /// `exclude_session` is the session id to skip (typically the active
    /// one).  Pass an empty string to include every session.
    ///
    /// Default impl returns an empty vec so in-memory / ephemeral backends
    /// keep compiling.  SurrealDB-backed stores override with a BM25 query.
    async fn search_session_memory(
        &self,
        _query:           &str,
        _exclude_session: &str,
        _limit:           usize,
    ) -> Result<Vec<SessionRecallHit>, MemoryError> { Ok(Vec::new()) }

    /// Compact the store: replace everything except the `keep_recent`
    /// newest entries with a single `summary` entry.  Returns the number
    /// of entries that were compacted away.
    ///
    /// Default impl is non-atomic (`recent` → `clear` → re-push) and
    /// suitable for in-memory backends.  Persistent backends should
    /// override with an atomic delete-then-insert to avoid losing
    /// history on partial failure.
    async fn compact(
        &self,
        keep_recent: usize,
        summary:     MemoryEntry,
    ) -> Result<usize, MemoryError> {
        let all = self.recent(usize::MAX).await?;
        if all.len() <= keep_recent {
            return Ok(0);
        }
        let cut       = all.len() - keep_recent;
        let tail      = all[cut..].to_vec();
        self.clear().await?;
        self.push(summary).await?;
        for e in tail {
            self.push(e).await?;
        }
        Ok(cut)
    }
}
