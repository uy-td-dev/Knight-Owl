//! Orchestra — shared types for the agent / skill / workflow / command system.
//!
//! These types are the **stable contract** between `owl-orchestra` (loader,
//! registry, engine), `owl-brain` (factory, runner), and `owl-desktop` (UI).
//! Schema bumps are governed by [`SCHEMA_VERSION`] and migration code in
//! `owl-orchestra::migrate`.
//!
//! Design axioms (see Design Spec v1 §0):
//! - A1 — every spec carries `schema_version`.
//! - A2 — `id` is immutable, `name` / `description` are mutable strings.
//! - A7 — failures are typed via [`OrchestraProtoError`], never panics.
//!
//! Allowed deps: `serde`, `serde_json`, `thiserror`, `schemars`. R-14 ✓.

pub mod ids;
pub mod agent;
pub mod skill;
pub mod workflow;
pub mod command;
pub mod error;

#[cfg(test)]
mod tests;

pub use agent::{AgentSpec, AgentBudget, ModelSpec, ProviderRef};
pub use command::{CommandExpansion, CommandSpec};
pub use error::OrchestraProtoError;
pub use ids::{
    AgentId, CommandId, ModelRef, SkillId, StepId, ToolName, TraceId, WorkflowId,
};
pub use skill::SkillSpec;
pub use workflow::{FailurePolicy, StepSpec, WorkflowSpec};

/// Highest spec schema version this crate understands.
///
/// Bumped on backwards-incompatible field changes.  Loaders MUST refuse files
/// with `schema_version > SCHEMA_VERSION` and tell the user to upgrade
/// Knight-Owl.  Older versions migrate forwards via `owl-orchestra::migrate`.
pub const SCHEMA_VERSION: u32 = 1;

/// Kinds of orchestra entities — used by registry events and error reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecKind {
    Agent,
    Skill,
    Workflow,
    Command,
}

impl std::fmt::Display for SpecKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SpecKind::Agent    => "agent",
            SpecKind::Skill    => "skill",
            SpecKind::Workflow => "workflow",
            SpecKind::Command  => "command",
        })
    }
}
