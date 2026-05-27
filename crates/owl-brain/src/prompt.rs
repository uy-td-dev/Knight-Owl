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

/// System prompt for skill-draft extraction.
///
/// Triggered by the distillation worker when a success cluster crosses
/// the skill threshold (≥ 5 successful tasks with similar request +
/// action sequence).  The model returns a JSON object that the worker
/// turns into a `SkillDraft` → on-disk `.md` skill file.
pub const SKILL_DISTILLATION_SYSTEM: &str = "\
You are a procedural-knowledge extractor. Given a cluster of successfully \
completed task memories that share a recurring approach, extract ONE reusable \
skill.

Respond with exactly this JSON (no prose, no markdown fences):
{
  \"name\":              \"short Title Case name\",
  \"description\":       \"one-sentence purpose, max 120 chars\",
  \"trigger\":           \"plain-English hint of when this skill applies\",
  \"recommended_tools\": [\"tool_a\", \"tool_b\"],
  \"body\":              \"## When\\n...\\n## Steps\\n1. ...\\n2. ...\"
}

Rules:
- name: imperative, no punctuation, e.g. \"Run Cargo Check After Edits\".
- body: Markdown. Open with `## When` (trigger conditions, 1-3 lines), \
  then `## Steps` (numbered, ≤ 7 steps). Each step a single sentence.
- recommended_tools: tool names that appear in the cluster's actions; \
  empty list is OK if the skill is pure reasoning.
- Do NOT invent capabilities outside what the cluster demonstrates.";

/// System prompt for the context compactor.
///
/// Used by [`crate::compactor::LlmCompactor`] to fold a stretch of old
/// conversation entries into one compact summary so the reasoning loop's
/// context window stays bounded over long-running sessions.
pub const COMPACTION_SYSTEM: &str = "\
You are a transcript compressor.  Given a chronological run of messages \
between a user, an assistant, and tools, produce a concise summary \
(target: ≤ 200 words) that preserves:

- The user's high-level goal(s) for this stretch of conversation.
- Concrete decisions made + tools invoked + their key results.
- Open items the assistant must still address.
- Identifiers (file paths, commit hashes, error codes) — KEEP these verbatim.

Do NOT:
- Paste raw tool output (refer to it abstractly: \"ran cargo check, 3 errors\").
- Repeat assistant prose at length (summarise the action, not the wording).
- Invent details that weren't in the transcript.

Reply with the summary text only — no preamble, no JSON, no markdown headers.";

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
