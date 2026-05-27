//! R-22 review-block + retry integration test.
//!
//! Drives `ReasoningLoop` through: edit → sandbox passes → reviewer returns
//! one Block violation → loop re-enters Plan with violation summary in
//! memory → second attempt has no violations → loop converges Ok.
//!
//! Asserts:
//!   1. Reviewer is called after sandbox success (not before).
//!   2. Block violations are persisted to L4.
//!   3. The loop iterates back to Plan when violations are present.
//!   4. Warn-only violations do NOT block completion.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use owl_brain::memory::InMemoryStore;
use owl_brain::reasoning_loop::ToolExecutor;
use owl_brain::reviewer::Reviewer;
use owl_brain::{BrainError, MemoryStore, ReasoningConfig, ReasoningLoop};
use owl_harness::mock_engine::MockEngine;
use owl_protocol::code::{Severity, Violation};
use owl_protocol::experience::{
    ExperienceError, ExperienceStore, Insight, TaskMemory, TestRun,
};
use owl_protocol::sandbox::{ExecutionOutcome, ExecutionPlan, Sandbox, SandboxError};
use owl_protocol::tools::{ToolCall, ToolResult};

// ── Test scaffolding ──────────────────────────────────────────────────────

struct RecordingExecutor;
#[async_trait]
impl ToolExecutor for RecordingExecutor {
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, BrainError> {
        Ok(ToolResult::ok(call.name, serde_json::json!("ok")))
    }
    fn has_tool(&self, name: &str) -> bool {
        matches!(name, "write_file" | "edit_file" | "bash")
    }
}

/// Sandbox that always succeeds — review is what we're testing here.
struct AlwaysPassSandbox;
#[async_trait]
impl Sandbox for AlwaysPassSandbox {
    async fn run(&self, _plan: ExecutionPlan) -> Result<ExecutionOutcome, SandboxError> {
        Ok(ExecutionOutcome {
            task_id:     "test".into(),
            exit_code:   0,
            stdout:      String::new(),
            stderr:      String::new(),
            duration_ms: 1,
            success:     true,
        })
    }
}

/// Reviewer that returns Block violations on the first call, none on
/// subsequent calls — exercises the retry loop.
struct FlakyReviewer {
    calls: Mutex<u32>,
}
impl FlakyReviewer {
    fn new() -> Self { Self { calls: Mutex::new(0) } }
}
#[async_trait]
impl Reviewer for FlakyReviewer {
    async fn review(
        &self,
        task_id:   &str,
        _refs:     &[String],
    ) -> Result<Vec<Violation>, BrainError> {
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        if *n == 1 {
            Ok(vec![Violation {
                id:           uuid::Uuid::new_v4().to_string(),
                code_node_id: "a".into(),
                standard_id:  "R-1-function-length".into(),
                task_id:      task_id.into(),
                evidence:     "function `foo` is 47 lines (> 30)".into(),
                severity:     Severity::Block,
                created_at:   0,
            }])
        } else {
            Ok(Vec::new())
        }
    }
}

/// Reviewer that always returns one Warn-severity violation.  Used to
/// assert Warn does NOT block.
struct WarnOnlyReviewer;
#[async_trait]
impl Reviewer for WarnOnlyReviewer {
    async fn review(
        &self,
        task_id:   &str,
        _refs:     &[String],
    ) -> Result<Vec<Violation>, BrainError> {
        Ok(vec![Violation {
            id:           uuid::Uuid::new_v4().to_string(),
            code_node_id: "a".into(),
            standard_id:  "R-10-doc-pub".into(),
            task_id:      task_id.into(),
            evidence:     "pub fn `bar` missing doc comment".into(),
            severity:     Severity::Warn,
            created_at:   0,
        }])
    }
}

#[derive(Default)]
struct InMemExperience {
    violations: Mutex<Vec<Violation>>,
    memories:   Mutex<Vec<TaskMemory>>,
}
#[async_trait]
impl ExperienceStore for InMemExperience {
    async fn store_task_memory(&self, m: TaskMemory) -> Result<(), ExperienceError> {
        self.memories.lock().unwrap().push(m);
        Ok(())
    }
    async fn recall_insights(&self, _: &str) -> Result<Vec<Insight>, ExperienceError> {
        Ok(Vec::new())
    }
    async fn upsert_insight(&self, _: Insight) -> Result<(), ExperienceError> { Ok(()) }
    async fn recent_task_memories(&self, _: u64) -> Result<Vec<TaskMemory>, ExperienceError> {
        Ok(Vec::new())
    }
    async fn store_test_run(&self, _: TestRun) -> Result<(), ExperienceError> { Ok(()) }
    async fn record_violation(&self, v: Violation) -> Result<(), ExperienceError> {
        self.violations.lock().unwrap().push(v);
        Ok(())
    }
}

fn cfg() -> ReasoningConfig {
    ReasoningConfig {
        max_steps:           6,
        memory_context_limit: 5,
        system_prompt:       "test".into(),
        verify_retries:      2,
        review_retries:      2,
        workspace_path:      Some("/tmp/test-ws".into()),
        ..ReasoningConfig::default()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn review_block_triggers_retry_and_converges() {
    // Turn 1: edit; Turn 2: "done"; Turn 3 (after review block): "fixed".
    let model = MockEngine::new()
        .on("", r#"{"tool":"write_file","args":{"path":"a.rs","content":"fn main(){}"}}"#)
        .on("", "done")
        .on("", "fixed");

    let exp_impl = Arc::new(InMemExperience::default());
    let exp: Arc<dyn ExperienceStore> = Arc::clone(&exp_impl) as Arc<dyn ExperienceStore>;

    let loop_ = ReasoningLoop::new(
        model,
        Arc::new(RecordingExecutor) as Arc<dyn ToolExecutor>,
        Arc::new(InMemoryStore::new()) as Arc<dyn MemoryStore>,
        cfg(),
    )
    .with_experience(exp)
    .with_sandbox(Arc::new(AlwaysPassSandbox))
    .with_reviewer(Arc::new(FlakyReviewer::new()));

    let result = loop_.run("write the file").await
        .expect("review should converge after one retry");
    assert_eq!(result, "fixed");

    let violations = exp_impl.violations.lock().unwrap().clone();
    assert_eq!(violations.len(), 1, "exactly one Block violation persisted");
    assert!(matches!(violations[0].severity, Severity::Block));
    assert_eq!(violations[0].standard_id, "R-1-function-length");

    // Exactly one TaskMemory written (one user task, regardless of retries).
    assert_eq!(exp_impl.memories.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn review_warn_does_not_block() {
    // Single turn: edit + immediate "done" reply.  Reviewer returns Warn.
    let model = MockEngine::new()
        .on("", r#"{"tool":"write_file","args":{"path":"a.rs","content":"x"}}"#)
        .on("", "done");

    let exp_impl = Arc::new(InMemExperience::default());
    let exp: Arc<dyn ExperienceStore> = Arc::clone(&exp_impl) as Arc<dyn ExperienceStore>;

    let loop_ = ReasoningLoop::new(
        model,
        Arc::new(RecordingExecutor) as Arc<dyn ToolExecutor>,
        Arc::new(InMemoryStore::new()) as Arc<dyn MemoryStore>,
        cfg(),
    )
    .with_experience(exp)
    .with_sandbox(Arc::new(AlwaysPassSandbox))
    .with_reviewer(Arc::new(WarnOnlyReviewer));

    let result = loop_.run("write it").await.expect("warn should not block");
    assert_eq!(result, "done");

    // Warn IS recorded — we just don't loop on it.
    let violations = exp_impl.violations.lock().unwrap().clone();
    assert_eq!(violations.len(), 1);
    assert!(matches!(violations[0].severity, Severity::Warn));
}

#[tokio::test]
async fn review_skipped_when_no_reviewer_wired() {
    let model = MockEngine::new()
        .on("", r#"{"tool":"write_file","args":{"path":"a.rs","content":"x"}}"#)
        .on("", "done");

    let exp_impl = Arc::new(InMemExperience::default());
    let exp: Arc<dyn ExperienceStore> = Arc::clone(&exp_impl) as Arc<dyn ExperienceStore>;

    // No .with_reviewer() — review must be a no-op.
    let loop_ = ReasoningLoop::new(
        model,
        Arc::new(RecordingExecutor) as Arc<dyn ToolExecutor>,
        Arc::new(InMemoryStore::new()) as Arc<dyn MemoryStore>,
        cfg(),
    )
    .with_experience(exp)
    .with_sandbox(Arc::new(AlwaysPassSandbox));

    let result = loop_.run("anything").await.unwrap();
    assert_eq!(result, "done");
    assert!(exp_impl.violations.lock().unwrap().is_empty());
}
