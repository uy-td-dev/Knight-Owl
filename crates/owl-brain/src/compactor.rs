//! Context compaction — keeps conversation memory bounded by replacing
//! the oldest N entries with one LLM-generated summary.
//!
//! Triggered by the reasoning loop's reflection block after every task:
//! when `memory.recent(usize::MAX).len() > compact_threshold`, the
//! configured `Compactor` summarizes everything older than
//! `compact_keep_recent` and the store atomically swaps them in.
//!
//! Mirror of Hermes' `/compress` but automated.

use std::sync::Arc;

use async_trait::async_trait;
use rig::agent::AgentBuilder;
use rig::completion::{CompletionModel, Prompt};

use owl_protocol::memory::MemoryEntry;

use crate::BrainError;

/// Pluggable compaction backend — injected into `ReasoningLoop` via
/// `with_compactor`.  Without one, the loop never auto-summarises and
/// memory keeps growing.
#[async_trait]
pub trait Compactor: Send + Sync {
    /// Produce a single summary entry that captures the salient state of
    /// `entries`.  The returned entry's role should be `"system"` so the
    /// model treats it as background context, not a user turn.
    async fn summarize(
        &self,
        entries: &[MemoryEntry],
    ) -> Result<MemoryEntry, BrainError>;
}

/// No-op compactor — used when no compactor is wired.  Never shrinks.
pub struct NoOpCompactor;

#[async_trait]
impl Compactor for NoOpCompactor {
    async fn summarize(
        &self,
        _entries: &[MemoryEntry],
    ) -> Result<MemoryEntry, BrainError> {
        Ok(MemoryEntry {
            role:    "system".into(),
            content: "(compaction disabled)".into(),
        })
    }
}

/// LLM-backed compactor — calls the model with the
/// [`crate::prompt::COMPACTION_SYSTEM`] preamble.
///
/// Generic over any `rig::completion::CompletionModel` so it works with
/// every provider Knight-Owl ships.
pub struct LlmCompactor<M>
where
    M: CompletionModel + Clone,
{
    model: Arc<M>,
}

impl<M> LlmCompactor<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    pub fn new(model: M) -> Self {
        Self { model: Arc::new(model) }
    }
}

#[async_trait]
impl<M> Compactor for LlmCompactor<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    async fn summarize(
        &self,
        entries: &[MemoryEntry],
    ) -> Result<MemoryEntry, BrainError> {
        let transcript = entries
            .iter()
            .map(|e| format!("{}: {}", e.role, e.content))
            .collect::<Vec<_>>()
            .join("\n");
        let agent = AgentBuilder::new((*self.model).clone())
            .preamble(crate::prompt::COMPACTION_SYSTEM)
            .build();
        let summary = agent
            .prompt(transcript.as_str())
            .await
            .map_err(|e| BrainError::Completion(e.to_string()))?;
        Ok(MemoryEntry {
            role:    "system".into(),
            content: format!("[compacted summary]\n{}", summary.trim()),
        })
    }
}
