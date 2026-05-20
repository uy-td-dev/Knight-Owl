//! Workflow specification — multi-step orchestration over agents.
//!
//! v1 engine ships a *sequential* executor that orders steps by topological
//! sort of `depends`.  The spec already supports DAG (`depends: Vec<StepId>`)
//! so a parallel executor can land later without spec churn.

use serde::{Deserialize, Serialize};

use super::ids::{AgentId, StepId, WorkflowId};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowSpec {
    /// Spec format version.
    pub schema_version: u32,

    // ── Identity ─────────────────────────────────────────────────────────────
    pub id:          WorkflowId,
    pub name:        String,
    pub description: String,

    /// Optional slash-command alias (`"/refactor"`).  When set, typing it in
    /// the chat input expands into a workflow run.
    pub trigger: Option<String>,

    /// Steps in declaration order.  Execution order is determined by
    /// topological sort of `StepSpec::depends`, NOT by Vec position — the
    /// engine MUST not assume Vec order is execution order.
    pub steps: Vec<StepSpec>,

    // ── Budget / failure handling ────────────────────────────────────────────
    /// Default failure policy for steps that don't override it.
    pub on_failure: FailurePolicy,
    /// Hard cap on total wall-clock duration.  Engine cancels if exceeded.
    pub timeout_ms: u64,
    /// Optional cap on aggregate tokens across all steps.
    pub max_total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StepSpec {
    /// Local id, unique within the workflow.  Referenced by other steps via
    /// `depends` and by template expansion (`{{<step_id>.output}}`).
    pub id: StepId,

    /// Agent that runs this step.
    pub agent: AgentId,

    /// Step ids this step depends on; must complete first.
    pub depends: Vec<StepId>,

    /// Prompt template — supports `{{user_input}}`, `{{<step>.output}}`,
    /// `{{<step>.json.<path>}}`, `{{env.NAME}}`.  Strict mode: unknown
    /// variable = error.
    pub prompt: String,

    /// Per-step override of the workflow-level [`FailurePolicy`].
    pub on_failure: Option<FailurePolicy>,
}

/// What the engine does when a step errors out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    /// Stop the workflow; mark it failed.
    Abort,
    /// Re-run the failing step exactly once; if it fails again, escalate.
    RetryOnce,
    /// Mark the step failed but keep running downstream steps that don't
    /// depend on it.  Use carefully — downstream prompts may reference an
    /// empty `output`.
    Continue,
}

impl Default for FailurePolicy {
    fn default() -> Self { Self::Abort }
}
