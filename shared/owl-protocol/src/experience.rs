//! L4 Experience layer — task memory, distilled insights, and the store trait.
//!
//! Lives in owl-protocol so owl-brain (caller) and owl-vault (impl) can share
//! the types without a circular dependency.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::code::Violation;
use crate::sandbox::ExecutionOutcome;

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
    /// Total prompt tokens consumed across every LLM turn in this task.
    /// `0` when the model provider didn't report usage (e.g. local Ollama).
    /// `#[serde(default)]` keeps older rows readable.
    #[serde(default)]
    pub input_tokens: u64,
    /// Total completion tokens consumed across every LLM turn.
    #[serde(default)]
    pub output_tokens: u64,
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

// ── TestRun (R-21 Sandbox-Verified Execute) ─────────────────────────────────

/// Record of a single sandbox verification run.
///
/// Persisted to L4 after every `Sandbox::run` call made by the reasoning
/// loop, regardless of success or failure.  Linked to the originating
/// `TaskMemory` via the `task_id` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRun {
    /// Unique id (UUID v4).
    pub id: String,
    /// Mirrors `TaskMemory::id` — links this verification to its task.
    pub task_id: String,
    /// `code_node` ids the verification was meant to check.
    pub code_refs: Vec<String>,
    /// Outcome returned by the sandbox.
    pub outcome: ExecutionOutcome,
    /// Unix millis when the run started.
    pub created_at: i64,
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

    /// Persist a sandbox verification run (R-21).
    ///
    /// Called by the reasoning loop after every `Sandbox::run` invocation.
    /// Default impl is a no-op so backends without L4 verification storage
    /// keep compiling.
    async fn store_test_run(&self, _run: TestRun) -> Result<(), ExperienceError> { Ok(()) }

    /// Fetch verification history for a given task, newest first.
    ///
    /// Used by the loop to decide whether the verify-retry budget is exhausted
    /// and by the distillation job to bias `AntiPattern` insights toward tasks
    /// that needed multiple retries to converge.
    async fn test_runs_for_task(
        &self,
        _task_id: &str,
    ) -> Result<Vec<TestRun>, ExperienceError> { Ok(Vec::new()) }

    /// Persist an R-22 review-phase violation (best-effort).
    async fn record_violation(&self, _v: Violation) -> Result<(), ExperienceError> { Ok(()) }

    /// Fetch every violation recorded for the given task.
    async fn violations_for_task(
        &self,
        _task_id: &str,
    ) -> Result<Vec<Violation>, ExperienceError> { Ok(Vec::new()) }

    /// Sum input + output tokens across every `TaskMemory` in a session.
    ///
    /// Returned as `(input_tokens, output_tokens)`.  Used by the future
    /// `/usage` slash command and by host UIs to surface cost dashboards.
    /// Default impl computes the sum from `recent_task_memories` —
    /// backends with native aggregation should override.
    async fn session_usage(
        &self,
        session_id: &str,
    ) -> Result<(u64, u64), ExperienceError> {
        let rows = self.recent_task_memories(10_000).await?;
        let (mut input, mut output) = (0u64, 0u64);
        for r in rows {
            if r.session_id == session_id {
                input  += r.input_tokens;
                output += r.output_tokens;
            }
        }
        Ok((input, output))
    }
}
