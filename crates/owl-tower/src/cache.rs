//! Prompt caching policy — Anthropic-specific request optimization.
//!
//! Anthropic's API supports `cache_control` markers on system prompts and
//! tool definitions; cached prefixes cost ~10% of normal input tokens after
//! the first request, with a 5-minute TTL.
//!
//! ## Activation status — **ACTIVE** (rig 0.36)
//!
//! Wired in [`crate::adapters::claude::build_claude_model`]: when
//! [`CachePolicy::Ephemeral`] is set (default), `with_automatic_caching()`
//! is enabled on the Anthropic completion model.  Every outgoing request
//! carries `cache_control: { "type": "ephemeral" }`; Anthropic's API
//! automatically places the cache breakpoint on the longest cacheable
//! prefix and reuses it across turns.
//!
//! ## What to cache
//!
//! High-value targets, in priority order:
//!
//! | Section | Reuse rate | Size | Cache priority |
//! |---------|------------|------|----------------|
//! | System prompt + skills | every turn | 5–20 KB | **HIGH** |
//! | Tool definitions       | every turn | 2–10 KB | **HIGH** |
//! | RAG context (pinned)   | per-task   | 1–5 KB  | medium         |
//! | Conversation history   | per-turn   | varies  | low (changes)  |
//!
//! [`AgentBuilder`]: rig::agent::AgentBuilder
//! [`CompletionModel`]: rig::completion::CompletionModel

/// Whether to inject Anthropic `cache_control` markers into outgoing requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CachePolicy {
    /// No cache markers; every request bills full input tokens.
    Off,
    /// Mark system prompt + tool definitions as `ephemeral` (5-min TTL).
    /// Recommended for any session with > 1 turn.
    #[default]
    Ephemeral,
}

/// Runtime cache state — paired with [`crate::config::Config::cache_policy`].
#[derive(Debug, Clone, Copy)]
pub struct PromptCache {
    pub policy:           CachePolicy,
    /// `true` when both the policy is non-Off AND the underlying rig version
    /// can actually emit cache markers.  Currently always `false` for rig 0.9.
    pub is_active:        bool,
}

impl PromptCache {
    /// Build a cache descriptor from a config policy.
    ///
    /// `runtime_supported` should be `true` only when the rig version in use
    /// can actually inject `cache_control` (rig ≥ 0.13).  Set to `false` for
    /// rig 0.9 — see crate-level docs.
    pub fn new(policy: CachePolicy, runtime_supported: bool) -> Self {
        Self {
            policy,
            is_active: runtime_supported && policy != CachePolicy::Off,
        }
    }

    /// Returns the static prefix portion of `system_prompt` that is safe to
    /// cache (everything before the dynamic-content marker, if any).
    ///
    /// Today this is a no-op pass-through; a future caller can split the
    /// prompt around a sentinel like `\n<dynamic>\n` so per-task additions
    /// don't break the cache key.
    pub fn split_for_cache<'a>(&self, system_prompt: &'a str) -> (&'a str, &'a str) {
        const SENTINEL: &str = "\n<dynamic>\n";
        match system_prompt.find(SENTINEL) {
            Some(idx) => (&system_prompt[..idx], &system_prompt[idx + SENTINEL.len()..]),
            None      => (system_prompt, ""),
        }
    }
}

/// Placeholder for the future `AgentBuilder` integration.
///
/// When `rig-core` exposes `cache_control` (≥ 0.13), implement this as:
/// ```ignore
/// builder.system_prompt_with_cache(prefix, CachePolicy::Ephemeral.into())
/// ```
/// Until then this function is a no-op preserving the existing behavior.
pub fn apply_to_agent_builder<B>(builder: B, _cache: PromptCache) -> B {
    // No-op for rig 0.9.  See module docs.
    builder
}
