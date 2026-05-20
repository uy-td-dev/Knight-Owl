//! Model size class — controls prompt selection + context budgets.
//!
//! Small local models (≤ 4B params) cannot keep up with the full
//! agentic prompt: they drown in long context, forget rules, and emit
//! malformed tool calls.  We auto-detect class from the model id and
//! dial-down memory, tool-result truncation, code-context chunks, and
//! prompt verbosity so they have a fighting chance.

/// Heuristic model size class derived from the model identifier string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelClass {
    /// Up to ~4B params — tiny prompts only, 1 tool/turn, tight budgets.
    Small,
    /// 4B – 14B — balanced budgets, can handle modest parallel tool calls.
    #[default]
    Medium,
    /// 14B+ or cloud (Claude / Gemini / GPT) — full prompt, full context.
    Large,
}

impl ModelClass {
    /// Classify by inspecting the model id.  Conservative — unrecognised
    /// ids fall back to [`Medium`] which is safe for most use cases.
    ///
    /// Heuristics (case-insensitive).  ORDER MATTERS: we check Large
    /// markers first because some Large model names happen to embed
    /// substrings that look like Small markers (e.g. `gemini` contains
    /// the substring "mini").
    pub fn from_model_id(id: &str) -> Self {
        let m = id.to_lowercase();

        // 1. Large — cloud providers (always Large regardless of suffix).
        let large_prefix = [
            "claude", "gpt-", "gpt_", "openai/", "gemini-", "gemini_",
            "deepseek-chat", "deepseek-coder-v2", "command-",
        ];
        if large_prefix.iter().any(|p| m.starts_with(p)) { return Self::Large; }

        // 2. Large — explicit big-param suffixes.
        let large_suffix = [":70b", ":72b", ":120b", ":405b", "-70b", "-72b"];
        if large_suffix.iter().any(|p| m.contains(p)) { return Self::Large; }

        // 3. Small — must use leading `:` or `-` so "mini" doesn't
        // accidentally match "gemini" (and `:2b` doesn't match "2.5").
        let small_suffix = [
            ":e2b", ":e4b", ":0.5b", ":1b", ":1.5b", ":2b", ":3b",
            "-e2b", "-e4b", "-1b", "-2b", "-3b",
            ":mini", "-mini",
            "tinyllama", "phi-3", "phi3:",
        ];
        if small_suffix.iter().any(|p| m.contains(p)) { return Self::Small; }

        // 4. Default → Medium (4–14B local, reasonable defaults).
        Self::Medium
    }

    /// Max memory entries pulled into context per turn.
    pub fn memory_limit(self) -> usize {
        match self { Self::Small => 5, Self::Medium => 15, Self::Large => 25 }
    }

    /// Maximum bytes of tool stdout/stderr forwarded back to the model.
    /// Small models can't reason over big blobs and waste context budget.
    pub fn tool_result_max_bytes(self) -> usize {
        match self { Self::Small => 800, Self::Medium => 4_000, Self::Large => 16_000 }
    }

    /// Max code_context chunks injected from the project graph.
    pub fn code_context_chunks(self) -> usize {
        match self { Self::Small => 3, Self::Medium => 8, Self::Large => 15 }
    }

    /// Whether to encourage parallel tool calls.  Tiny models barely emit
    /// one valid JSON tool call; asking for parallelism makes things worse.
    pub fn allow_parallel_tools(self) -> bool {
        !matches!(self, Self::Small)
    }

    /// Short human label for logs / UI.
    pub fn label(self) -> &'static str {
        match self { Self::Small => "small", Self::Medium => "medium", Self::Large => "large" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_models_detected() {
        for id in ["gemma4:e2b", "llama3.2:1b", "qwen2.5:1.5b", "phi3:mini", "tinyllama"] {
            assert_eq!(ModelClass::from_model_id(id), ModelClass::Small, "{id}");
        }
    }

    #[test]
    fn large_models_detected() {
        for id in ["claude-sonnet-4-6", "gpt-4o", "gemini-2.5-flash", "llama3.1:70b"] {
            assert_eq!(ModelClass::from_model_id(id), ModelClass::Large, "{id}");
        }
    }

    #[test]
    fn medium_default() {
        for id in ["qwen2.5-coder:7b", "llama3.1:8b", "unknown-model", ""] {
            assert_eq!(ModelClass::from_model_id(id), ModelClass::Medium, "{id}");
        }
    }

    #[test]
    fn budgets_scale_with_class() {
        assert!(ModelClass::Small.memory_limit()         < ModelClass::Medium.memory_limit());
        assert!(ModelClass::Small.tool_result_max_bytes() < ModelClass::Medium.tool_result_max_bytes());
        assert!(ModelClass::Small.code_context_chunks()  < ModelClass::Medium.code_context_chunks());
        assert!(!ModelClass::Small.allow_parallel_tools());
        assert!( ModelClass::Medium.allow_parallel_tools());
    }
}
