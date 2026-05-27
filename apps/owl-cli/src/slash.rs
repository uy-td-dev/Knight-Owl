//! In-chat slash commands — Hermes-style meta-controls.
//!
//! When the user's message starts with `/`, the chat command routes to
//! [`SlashDispatcher::dispatch`] instead of the LLM.  Commands return a
//! string that is printed verbatim (or formatted JSON when `--json`).
//!
//! Current commands:
//!   - `/help`               — list every command + one-liner
//!   - `/usage`              — token totals for the current session
//!   - `/skills`             — list every skill the orchestra registry knows
//!   - `/model`              — print the active model id
//!   - `/clear`              — wipe the current session's memory
//!   - `/compact`            — force-run compaction now (skipped if no compactor)
//!
//! Adding a command = one match arm + one doc-line in `/help`.

use std::sync::Arc;

use anyhow::{anyhow, Result};

use owl_protocol::experience::ExperienceStore;
use owl_protocol::memory::MemoryStore;

/// Pluggable backend for slash commands — collects every dependency
/// the built-in commands need.
pub struct SlashDispatcher {
    pub session_id:    String,
    pub model_id:      String,
    pub memory:        Arc<dyn MemoryStore>,
    pub experience:    Option<Arc<dyn ExperienceStore>>,
}

/// Outcome of running a slash command.
#[derive(Debug, Clone)]
pub struct SlashOutput {
    /// Text the CLI prints to stdout.
    pub message: String,
}

impl SlashDispatcher {
    /// Run the command encoded in `line` (without the leading `/`).
    ///
    /// `/usage`               -> "usage"
    /// `/model claude-opus`   -> ("model", "claude-opus")
    pub async fn dispatch(&self, line: &str) -> Result<SlashOutput> {
        let line = line.trim();
        let (name, args) = match line.split_once(char::is_whitespace) {
            Some((n, rest)) => (n, rest.trim()),
            None            => (line, ""),
        };

        let message = match name {
            "help"    => help_text(),
            "usage"   => self.cmd_usage().await?,
            "skills"  => self.cmd_skills().await?,
            "model"   => self.cmd_model(args)?,
            "clear"   => self.cmd_clear().await?,
            "compact" => self.cmd_compact().await?,
            other     => return Err(anyhow!(
                "unknown slash command: /{other}\nrun /help to see the list"
            )),
        };
        Ok(SlashOutput { message })
    }

    async fn cmd_usage(&self) -> Result<String> {
        let exp = self.experience.as_ref()
            .ok_or_else(|| anyhow!("no persistent experience store; run with --workspace"))?;
        let (input, output) = exp.session_usage(&self.session_id).await
            .map_err(|e| anyhow!("session_usage: {e}"))?;
        Ok(format!(
            "session: {}\n  input  tokens: {input}\n  output tokens: {output}\n  total:         {}",
            self.session_id,
            input + output,
        ))
    }

    async fn cmd_skills(&self) -> Result<String> {
        // We could read from owl-orchestra here, but adding that dep just
        // for a listing is overkill — list the disk paths instead so the
        // user can inspect.  When the orchestra registry exposes a
        // listing API publicly we can swap to that.
        let cwd = std::env::current_dir().ok();
        let mut lines = vec!["skills directories:".to_string()];
        if let Some(cwd) = &cwd {
            lines.push(format!("  workspace: {}", cwd.join(".knight-owl/skills").display()));
        }
        if let Ok(home) = std::env::var("HOME") {
            lines.push(format!("  global:    {}/.knight-owl/skills", home));
        }
        // Inline a count by globbing.
        if let Some(cwd) = &cwd {
            let dir = cwd.join(".knight-owl/skills");
            if let Ok(rd) = std::fs::read_dir(&dir) {
                let n = rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
                    .count();
                lines.push(format!("  workspace count: {n} skill(s)"));
            }
        }
        Ok(lines.join("\n"))
    }

    fn cmd_model(&self, args: &str) -> Result<String> {
        if !args.is_empty() {
            // Switching is provider-specific and requires re-wiring the
            // tower adapter.  For Phase C we only surface the current id.
            return Err(anyhow!(
                "/model switching not yet supported — start a new session with --provider"
            ));
        }
        Ok(format!("active model: {}", self.model_id))
    }

    async fn cmd_clear(&self) -> Result<String> {
        self.memory.clear().await
            .map_err(|e| anyhow!("clear: {e}"))?;
        Ok(format!("cleared memory for session {}", self.session_id))
    }

    async fn cmd_compact(&self) -> Result<String> {
        // Manual /compact intentionally doesn't run the LLM compactor
        // here — that would require an `Arc<dyn Compactor>` plumbed in,
        // which only the chat command's `build_and_run!` macro has.
        // Surface this as a hint instead so the user knows auto-compact
        // is already wired.
        let total = self.memory.recent(usize::MAX).await
            .map_err(|e| anyhow!("recent: {e}"))?
            .len();
        Ok(format!(
            "memory size: {total} entries — auto-compaction fires above ReasoningConfig::compact_threshold (default 80)"
        ))
    }
}

fn help_text() -> String {
    [
        "Available slash commands:",
        "  /help     this message",
        "  /usage    token totals for the current session",
        "  /skills   list skill directories + count",
        "  /model    print the active model id",
        "  /clear    wipe the current session's memory",
        "  /compact  show memory size + auto-compaction threshold",
    ].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use owl_brain::memory::InMemoryStore;
    use owl_protocol::experience::{
        ExperienceError, Insight, TaskMemory, TestRun,
    };

    #[derive(Default)]
    struct StubExperience {
        memories: Vec<TaskMemory>,
    }
    #[async_trait]
    impl ExperienceStore for StubExperience {
        async fn store_task_memory(&self, _: TaskMemory) -> Result<(), ExperienceError> { Ok(()) }
        async fn recall_insights(&self, _: &str) -> Result<Vec<Insight>, ExperienceError> { Ok(vec![]) }
        async fn upsert_insight(&self, _: Insight) -> Result<(), ExperienceError> { Ok(()) }
        async fn recent_task_memories(&self, _: u64) -> Result<Vec<TaskMemory>, ExperienceError> {
            Ok(self.memories.clone())
        }
        async fn store_test_run(&self, _: TestRun) -> Result<(), ExperienceError> { Ok(()) }
    }

    fn dispatcher() -> SlashDispatcher {
        SlashDispatcher {
            session_id: "test-sess".into(),
            model_id:   "claude-test".into(),
            memory:     Arc::new(InMemoryStore::new()),
            experience: Some(Arc::new(StubExperience::default())),
        }
    }

    #[tokio::test]
    async fn help_lists_commands() {
        let out = dispatcher().dispatch("help").await.unwrap();
        assert!(out.message.contains("/usage"));
        assert!(out.message.contains("/skills"));
    }

    #[tokio::test]
    async fn unknown_command_errors() {
        let err = dispatcher().dispatch("frobnicate").await.unwrap_err();
        assert!(err.to_string().contains("unknown"));
    }

    #[tokio::test]
    async fn usage_reports_zero_for_empty_session() {
        let out = dispatcher().dispatch("usage").await.unwrap();
        assert!(out.message.contains("input  tokens: 0"));
        assert!(out.message.contains("output tokens: 0"));
    }

    #[tokio::test]
    async fn usage_sums_only_matching_session() {
        let exp = StubExperience {
            memories: vec![
                TaskMemory {
                    id: "1".into(),
                    request: "x".into(),
                    actions: vec![],
                    outcome: owl_protocol::experience::TaskOutcome::Success,
                    code_refs: vec![],
                    session_id: "test-sess".into(),
                    input_tokens:  10,
                    output_tokens: 5,
                },
                TaskMemory {
                    id: "2".into(),
                    request: "y".into(),
                    actions: vec![],
                    outcome: owl_protocol::experience::TaskOutcome::Success,
                    code_refs: vec![],
                    session_id: "OTHER".into(),
                    input_tokens:  999,
                    output_tokens: 999,
                },
            ],
        };
        let mut d = dispatcher();
        d.experience = Some(Arc::new(exp));
        let out = d.dispatch("usage").await.unwrap();
        assert!(out.message.contains("input  tokens: 10"));
        assert!(out.message.contains("output tokens: 5"));
        assert!(!out.message.contains("999"));
    }

    #[tokio::test]
    async fn clear_returns_ok_message() {
        let out = dispatcher().dispatch("clear").await.unwrap();
        assert!(out.message.contains("cleared"));
    }

    #[tokio::test]
    async fn model_prints_active_id() {
        let out = dispatcher().dispatch("model").await.unwrap();
        assert!(out.message.contains("claude-test"));
    }

    #[tokio::test]
    async fn model_switch_not_supported() {
        let err = dispatcher().dispatch("model gpt-4").await.unwrap_err();
        assert!(err.to_string().contains("not yet supported"));
    }
}
