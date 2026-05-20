//! L4 Experience layer — task memory, distilled insights, and the store trait.
//!
//! Lives in owl-protocol so owl-brain (caller) and owl-vault (impl) can share
//! the types without a circular dependency.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── TaskMemory ──────────────────────────────────────────────────────────────

/// Record of a single completed task — written to L4 after every run (R-22).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMemory {
    /// Unique id (UUID v4).
    pub id: String,
    /// The original user request string.
    pub request: String,
    /// Ordered list of tool calls made during the task.
    pub actions: Vec<String>,
    /// Terminal outcome — success or failure with diagnostics.
    pub outcome: TaskOutcome,
    /// `code_node` ids touched during the task.
    pub code_refs: Vec<String>,
    /// Session id linking to `session_memory` conversation rows.
    pub session_id: String,
}

/// Terminal state of a completed task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskOutcome {
    Success,
    Failure {
        reason: String,
        /// Last 1 KiB of stderr / compiler output for distillation.
        stderr_tail: String,
    },
}

// ── Insight ─────────────────────────────────────────────────────────────────

/// Distilled knowledge derived from clustered task memories (R-22).
///
/// Written exclusively by the distillation job — never by handwritten code paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Insight {
    /// Unique id (UUID v4).
    pub id: String,
    /// Pattern, anti-pattern, or project rule.
    pub kind: InsightKind,
    /// Scope: `"global"` | `"crate:<name>"` | `"file:<path>"`.
    pub scope: String,
    /// Short human-readable summary of the insight.
    pub summary: String,
    /// `TaskMemory` ids that constitute evidence for this insight.
    pub evidence: Vec<String>,
}

/// Classification of a distilled insight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InsightKind {
    /// A recurring successful fix pattern — repeat it.
    Pattern,
    /// A recurring failure pattern — avoid it.
    AntiPattern,
    /// A project-specific invariant crystallised from repeated failures.
    Rule,
}

// ── ExperienceStore trait ────────────────────────────────────────────────────

/// Error type for [`ExperienceStore`] operations.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ExperienceError {
    #[error("experience store error: {0}")]
    Store(String),
}

/// Pluggable L4 experience backend.
///
/// Trait lives in protocol so owl-brain can hold `Arc<dyn ExperienceStore>`
/// without importing owl-vault directly (R-13).
///
/// Concrete implementation: `owl_vault::SurrealExperienceStore`.
#[async_trait]
pub trait ExperienceStore: Send + Sync {
    /// Persist a task memory row immediately after task completion.
    async fn store_task_memory(&self, memory: TaskMemory) -> Result<(), ExperienceError>;

    /// Return insights whose scope matches `scope_prefix`.
    ///
    /// `scope_prefix` can be `"global"`, `"crate:owl-brain"`, or `"file:src/lib.rs"`.
    async fn recall_insights(&self, scope_prefix: &str) -> Result<Vec<Insight>, ExperienceError>;

    /// Upsert a distilled insight (called only from distillation jobs).
    async fn upsert_insight(&self, insight: Insight) -> Result<(), ExperienceError>;

    /// Fetch the most-recent task memories for distillation clustering.
    async fn recent_task_memories(&self, limit: u64) -> Result<Vec<TaskMemory>, ExperienceError>;

    /// Wipe every insight row (used by the desktop "Clear insights"
    /// dev command when accumulated insights start biasing the agent).
    /// Default impl returns 0 so older backends compile unchanged.
    async fn clear_insights(&self) -> Result<u64, ExperienceError> { Ok(0) }
}
