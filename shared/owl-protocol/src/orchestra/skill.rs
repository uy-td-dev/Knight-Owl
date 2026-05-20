//! Skill specification — a reusable capability bundle composed into an
//! agent's system prompt.
//!
//! In the v1 design (Design Spec §4), skills are loaded **eagerly**: every
//! skill listed by `agent.skills` is appended to the agent's system prompt at
//! spawn time.  The `trigger` field is informational only — it documents
//! when the skill is intended to fire but does not gate runtime activation.
//! Lazy / on-demand skill activation may arrive in a later schema version.

use serde::{Deserialize, Serialize};

use super::ids::{SkillId, ToolName};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SkillSpec {
    /// Spec format version.
    pub schema_version: u32,

    // ── Identity ─────────────────────────────────────────────────────────────
    pub id:          SkillId,
    pub name:        String,
    pub description: String,

    /// Free-text hint describing when this skill is intended to fire.
    /// Currently informational; reserved for lazy activation in later versions.
    pub trigger: Option<String>,

    /// Tools the skill author expects the agent to use.  This is a HINT —
    /// it does NOT widen [`super::AgentSpec::allowed_tools`].  Tool access
    /// remains gated by the agent's allowlist.
    pub recommended_tools: Vec<ToolName>,

    /// Markdown body — the actual instruction text composed into the prompt.
    pub body: String,
}
