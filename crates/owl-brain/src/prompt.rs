//! System prompt for the Knight-Owl reasoning loop.
//!
//! Two template files:
//! - `prompts/system.md`       — full prompt for Medium/Large models
//! - `prompts/system_small.md` — minimal prompt for ≤ 4B models
//!
//! Pick at construction time via [`system_for`].

use crate::model_class::ModelClass;

/// System prompt for Medium/Large models — full agentic instructions.
pub const SYSTEM:       &str = include_str!("../prompts/system.md");

/// System prompt for Small models — minimal surface, single-tool turns.
pub const SYSTEM_SMALL: &str = include_str!("../prompts/system_small.md");

/// Pick the right system prompt for a model class.
pub fn system_for(class: ModelClass) -> &'static str {
    match class {
        ModelClass::Small => SYSTEM_SMALL,
        _                 => SYSTEM,
    }
}

/// System prompt for the distillation worker (L4 insight extraction).
pub const DISTILLATION_SYSTEM: &str = "\
You are a code-assistant analyst. Given a cluster of completed task memories \
(each with a request, actions taken, and outcome), distill ONE insight.

Respond with exactly this JSON (no prose):
{
  \"kind\": \"Pattern\" | \"AntiPattern\" | \"Rule\",
  \"scope\": \"global\" | \"crate:<name>\" | \"file:<path>\",
  \"summary\": \"one-sentence actionable insight\"
}

Rules:
- Pattern: a recurring successful approach worth repeating.
- AntiPattern: a recurring failure pattern to avoid.
- Rule: a project invariant derived from repeated outcomes.
- scope: infer from code_refs. If all refs are in one crate \u{2192} crate:<name>. \
  If all in one file \u{2192} file:<path>. Otherwise \u{2192} global.
- summary: imperative voice, specific, max 120 chars.";
