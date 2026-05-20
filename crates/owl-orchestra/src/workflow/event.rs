//! Event stream emitted while a workflow runs.
//!
//! The shape is **stable** (Design Spec §11) — harness baseline traces
//! depend on it.  Add new fields only as `Option` / `#[serde(default)]` so
//! older recordings still parse.
//!
//! Numeric timestamps are `u64` ms-since-epoch — easier to diff than
//! `chrono::DateTime` and survive serde round-trips losslessly.

use serde::{Deserialize, Serialize};

use owl_protocol::orchestra::{AgentId, StepId, TraceId, WorkflowId};

/// Aggregate token usage for a step or workflow.
///
/// Currently zero-init at all sites — token tracking lands in a later phase.
/// Defined here so the shape doesn't need to change when it does.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens:  u64,
    pub output_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 { self.input_tokens + self.output_tokens }

    pub fn add(&mut self, other: TokenUsage) {
        self.input_tokens  += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_id:     StepId,
    pub agent:       AgentId,
    pub status:      StepStatus,
    pub output:      String,
    pub error:       Option<String>,
    pub tokens:      TokenUsage,
    pub started_at:  u64,
    pub finished_at: u64,
    /// Number of attempts taken to reach this status (1 = first try, 2 = one retry).
    pub attempts:    u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOutcome {
    pub trace_id:     TraceId,
    pub workflow_id:  WorkflowId,
    pub final_text:   String,
    pub step_results: Vec<StepResult>,
    pub tokens:       TokenUsage,
    pub started_at:   u64,
    pub finished_at:  u64,
    /// True if every step reached `Completed`.  False if any failed under
    /// `FailurePolicy::Continue`, the workflow was cancelled, or aborted.
    pub success:      bool,
}

/// Progress events streamed during execution.  Stable wire shape — see
/// `Design Spec §11` and `R-23 Spec Determinism`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowEvent {
    WorkflowStarted   { trace_id: TraceId, workflow: WorkflowId },
    StepStarted       { trace_id: TraceId, step: StepId, agent: AgentId, prompt: String },
    StepCompleted     { trace_id: TraceId, step: StepId, result: StepResult },
    StepFailed        { trace_id: TraceId, step: StepId, error: String },
    StepSkipped       { trace_id: TraceId, step: StepId, reason: String },
    WorkflowCompleted { outcome: WorkflowOutcome },
    WorkflowCancelled { trace_id: TraceId, workflow: WorkflowId },
}
