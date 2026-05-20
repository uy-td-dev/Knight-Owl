//! Agent specification — persona + capability surface + budget.
//!
//! An [`AgentSpec`] is the resolved, ready-to-run shape of an agent: any
//! cross-references (skills, allowed tools) have already been validated by
//! the loader / registry.  Constructing one directly bypasses that step, so
//! prefer `owl-orchestra::loader` for production paths.

use serde::{Deserialize, Serialize};

use super::ids::{AgentId, ModelRef, SkillId, ToolName};

/// The fully-resolved specification of an agent.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentSpec {
    /// Spec format version this entity was authored against.
    pub schema_version: u32,

    // ── Identity (immutable once published) ──────────────────────────────────
    /// Stable identifier; matches the folder name under `.knight-owl/agents/`.
    pub id: AgentId,
    /// Display name shown in the UI; safe to change without breaking refs.
    pub name: String,
    /// One-line description shown in agent picker / docs.
    pub description: String,

    // ── Execution config ─────────────────────────────────────────────────────
    /// Optional model override.  When `None`, the agent inherits the active
    /// session's model (configured globally via `owl-tower`).
    pub model: Option<ModelSpec>,
    /// System prompt **after** skill composition — produced by
    /// `owl-orchestra::compose::compose_system_prompt`.  Loader stores the raw
    /// `system.md` body separately; this field is the runtime artifact.
    pub system_prompt: String,
    /// Maximum reasoning iterations before the loop aborts.
    pub max_steps: usize,

    // ── Capability surface ───────────────────────────────────────────────────
    /// Tools this agent is permitted to call.  Empty vec = ALL tools allowed
    /// (back-compat default — matches pre-orchestra behaviour).
    pub allowed_tools: Vec<ToolName>,
    /// Skills attached to this agent (by id).  Loader resolves these to
    /// [`super::skill::SkillSpec`] and feeds them into prompt composition.
    pub skills: Vec<SkillId>,

    // ── Recursion ────────────────────────────────────────────────────────────
    /// Whether this agent is permitted to call the `spawn_agent` tool.
    pub can_spawn: bool,
    /// Maximum sub-agent recursion depth from this agent (clamped 0..=3).
    pub max_depth: u8,

    // ── Budget ───────────────────────────────────────────────────────────────
    /// Optional hard cap on total input tokens this agent may consume.
    pub max_input_tokens: Option<u64>,
    /// Optional hard cap on total output tokens this agent may produce.
    pub max_output_tokens: Option<u64>,
}

/// Model selection for an agent — provider routing + concrete model id.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelSpec {
    /// Which provider in `owl-tower` should serve this agent.
    pub provider: ProviderRef,
    /// Provider-specific model identifier (e.g. `gemini-2.5-flash`).
    pub id: ModelRef,
}

/// Provider routing — `Inherit` means "use whatever the session is configured
/// with"; the others select a specific adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRef {
    /// Use the session-default provider (whatever `owl-tower::Config` says).
    Inherit,
    Gemini,
    Anthropic,
    Ollama,
}

impl Default for ProviderRef {
    fn default() -> Self { Self::Inherit }
}

/// Optional resource budget; carried separately so it can be raised /
/// lowered at runtime without rebuilding the whole [`AgentSpec`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentBudget {
    pub max_input_tokens:  Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub max_depth:         Option<u8>,
}
