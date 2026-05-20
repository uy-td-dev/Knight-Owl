//! Errors raised while executing a workflow.
//!
//! Loader-time validation (unknown step deps, cycles) lives in
//! [`owl_protocol::orchestra::OrchestraProtoError`] — by the time the engine
//! sees a `WorkflowSpec` those have already been rejected.  This enum
//! captures runtime issues only.

use owl_protocol::orchestra::{AgentId, StepId, WorkflowId};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("workflow `{0}` not found in registry")]
    WorkflowNotFound(WorkflowId),

    #[error("step `{step}` references unknown agent `{agent}`")]
    AgentNotFound { step: StepId, agent: AgentId },

    #[error("template error in step `{step}`: {message}")]
    Template { step: StepId, message: String },

    #[error("step `{step}` failed after retries: {reason}")]
    StepFailed { step: StepId, reason: String },

    #[error("workflow timeout exceeded ({0} ms)")]
    Timeout(u64),

    #[error("workflow cancelled by caller")]
    Cancelled,

    #[error("token budget exceeded: used {used}, cap {cap}")]
    BudgetExceeded { used: u64, cap: u64 },

    #[error("brain error: {0}")]
    Brain(#[from] owl_brain::BrainError),
}
