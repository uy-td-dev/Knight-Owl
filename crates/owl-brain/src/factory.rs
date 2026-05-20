//! AgentFactory — injectable seam for building [`AgentRunner`] instances
//! from an [`owl_protocol::orchestra::AgentSpec`].
//!
//! `owl-brain` cannot construct concrete LLM clients (R-13: `owl-brain` must
//! not import `owl-tower`).  Therefore the factory contract lives here as a
//! trait, and the host application (`owl-desktop`, harness tests, …) supplies
//! the implementation that knows how to wire spec → model → executor.
//!
//! # Why this trait, not a closure
//!
//! - The factory is held as `Arc<dyn AgentFactory>` and cloned into many
//!   call sites (sub-agent tools, workflow engine).  Trait objects compose
//!   cleanly with `Arc`.
//! - Implementations may carry async state (model pool, tool registry); a
//!   raw `Fn` would force interior mutability everywhere.
//! - Mocking in `owl-harness` is straightforward: one struct, one impl.

use std::sync::Arc;

use async_trait::async_trait;
use owl_protocol::orchestra::{AgentSpec, SkillSpec};

use crate::error::BrainError;
use crate::runner::AgentRunner;

/// Build a runnable agent from a resolved spec + composed skills.
///
/// `depth` is the nesting level — root agent = 0, first sub-agent = 1,
/// and so on.  Implementations MUST refuse to build when
/// `depth > spec.max_depth` to keep recursion bounded (Design Spec §0 A4).
#[async_trait]
pub trait AgentFactory: Send + Sync {
    async fn build(
        &self,
        spec:   Arc<AgentSpec>,
        skills: Vec<Arc<SkillSpec>>,
        depth:  u8,
    ) -> Result<Arc<dyn AgentRunner>, BrainError>;
}

/// Convenience: a no-op factory used in tests where we never actually spawn
/// a runner.  Always returns [`BrainError::Config`] — kept here so test
/// crates don't have to roll their own.
pub struct NullFactory;

#[async_trait]
impl AgentFactory for NullFactory {
    async fn build(
        &self,
        _spec:   Arc<AgentSpec>,
        _skills: Vec<Arc<SkillSpec>>,
        _depth:  u8,
    ) -> Result<Arc<dyn AgentRunner>, BrainError> {
        Err(BrainError::Config(
            "NullFactory cannot build runners — wire a real factory in the host crate".into(),
        ))
    }
}
