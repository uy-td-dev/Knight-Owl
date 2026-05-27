//! R-22 Review phase — checks edited `code_node`s against `standard_node`
//! rules and emits blocking [`Violation`]s.
//!
//! The reasoning loop calls `Reviewer::review(...)` after sandbox verification
//! passes.  If any violations are returned with [`Severity::Block`], the
//! loop pushes them as a memory entry and re-enters Plan; otherwise the
//! task completes.

use async_trait::async_trait;

use owl_protocol::code::Violation;

use crate::BrainError;

/// Pluggable review backend — injected into `ReasoningLoop` via
/// `with_reviewer`.
///
/// The default in `owl-brain` is [`NoOpReviewer`], which approves every
/// task.  Production code wires either a `RuleBasedReviewer`
/// (regex / line-count checks over `code_node.preview`) or a
/// `SandboxedClippyReviewer` (runs `cargo clippy` and parses output) —
/// both deferred to a follow-up PR.
#[async_trait]
pub trait Reviewer: Send + Sync {
    /// Inspect the given `code_node` ids and return any violations found.
    ///
    /// `task_id` is the [`owl_protocol::experience::TaskMemory`] id so the
    /// reviewer can stamp each violation with the originating task.
    async fn review(
        &self,
        task_id:   &str,
        code_refs: &[String],
    ) -> Result<Vec<Violation>, BrainError>;
}

/// No-op reviewer — used when no reviewer is wired.  Never blocks.
pub struct NoOpReviewer;

#[async_trait]
impl Reviewer for NoOpReviewer {
    async fn review(
        &self,
        _task_id:   &str,
        _code_refs: &[String],
    ) -> Result<Vec<Violation>, BrainError> {
        Ok(Vec::new())
    }
}
