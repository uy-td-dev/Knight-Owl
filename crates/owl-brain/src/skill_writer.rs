//! Auto-skill generation — the bridge between distillation (Pattern
//! clusters) and the orchestra's on-disk skill library.
//!
//! Distillation produces [`SkillDraft`]s after every N tasks; a
//! [`SkillWriter`] persists them as `.knight-owl/skills/<id>.md` files
//! with the frontmatter format `owl-orchestra::loader` expects.
//!
//! This is the procedural-memory leg of Hermes-style self-improvement:
//! the agent doesn't just remember WHAT it did (TaskMemory) or
//! WHAT-TO-AVOID (Insight) — it crystallises HOW into reusable skills.

use async_trait::async_trait;

use crate::BrainError;

/// In-memory representation of a generated skill, ready to be written to
/// disk by a [`SkillWriter`] impl.
///
/// Mirrors `owl_protocol::orchestra::SkillSpec` field-for-field but lives
/// here so `owl-brain` can mint drafts without depending on the orchestra
/// crate (R-13 keeps brain → orchestra arrow empty).
#[derive(Debug, Clone)]
pub struct SkillDraft {
    /// Stable id — usually `"auto-<short-uuid>"`.  The writer is free to
    /// suffix-dedupe if a file with this id already exists.
    pub id: String,
    /// Human-readable name surfaced in `/skills` listings.
    pub name: String,
    /// One-line description.
    pub description: String,
    /// Free-text trigger hint (informational only, per current schema).
    pub trigger: Option<String>,
    /// Tool names the skill expects to use.  Hint only — does not widen
    /// the agent's allowlist.
    pub recommended_tools: Vec<String>,
    /// Markdown body — the actual instruction text that will be composed
    /// into the agent's system prompt at spawn time.
    pub body: String,
}

/// Persists [`SkillDraft`]s — typically as `.md` files on disk, but the
/// trait is abstract so tests can inject in-memory recorders and future
/// backends (e.g. uploading to a Skills Hub) can plug in.
#[async_trait]
pub trait SkillWriter: Send + Sync {
    /// Persist `draft` and return the final id (may differ from
    /// `draft.id` if dedupe / suffixing happened).
    async fn write_skill(&self, draft: SkillDraft) -> Result<String, BrainError>;
}

/// No-op writer — used when no writer is wired.  Discards every draft
/// and returns its id unchanged.  Lets the distillation worker remain
/// safe to call in test contexts without filesystem side-effects.
pub struct NoOpSkillWriter;

#[async_trait]
impl SkillWriter for NoOpSkillWriter {
    async fn write_skill(&self, draft: SkillDraft) -> Result<String, BrainError> {
        Ok(draft.id)
    }
}
