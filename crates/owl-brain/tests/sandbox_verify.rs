//! R-21 verify+retry loop integration test.
//!
//! Drives `ReasoningLoop` with a scripted model that emits one `write_file`
//! tool call followed by a plain-text reply, and a mock sandbox that fails
//! the first verification and succeeds the second.  Asserts the loop:
//!   1. Re-enters Plan with stderr fed back as a memory entry,
//!   2. Persists one `TestRun` row per sandbox invocation,
//!   3. Returns the final assistant text on second-attempt success.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use owl_brain::memory::InMemoryStore;
use owl_brain::reasoning_loop::ToolExecutor;
use owl_brain::{BrainError, MemoryStore, ReasoningConfig, ReasoningLoop};
use owl_harness::mock_engine::MockEngine;
use owl_protocol::experience::{
    ExperienceError, ExperienceStore, Insight, TaskMemory, TestRun,
};
use owl_protocol::sandbox::{ExecutionOutcome, ExecutionPlan, Sandbox, SandboxError};
use owl_protocol::tools::{ToolCall, ToolResult};

/// Records every tool call; always succeeds with a stub result.
struct RecordingExecutor {
    calls: Mutex<Vec<String>>,
}
impl RecordingExecutor {
    fn new() -> Self { Self { calls: Mutex::new(Vec::new()) } }
}
#[async_trait]
impl ToolExecutor for RecordingExecutor {
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, BrainError> {
        self.calls.lock().unwrap().push(call.name.clone());
        Ok(ToolResult::ok(call.name, serde_json::json!("ok")))
    }
    fn has_tool(&self, name: &str) -> bool {
        matches!(name, "write_file" | "edit_file" | "bash")
    }
}

/// Sandbox that returns Failure on the first call and Success on the second.
/// Records both invocations so the test can assert the retry happened.
struct FlakySandbox {
    invocations: Mutex<u32>,
}
impl FlakySandbox {
    fn new() -> Self { Self { invocations: Mutex::new(0) } }
}
#[async_trait]
impl Sandbox for FlakySandbox {
    async fn run(&self, _plan: ExecutionPlan) -> Result<ExecutionOutcome, SandboxError> {
        let mut n = self.invocations.lock().unwrap();
        *n += 1;
        let success = *n >= 2;
        Ok(ExecutionOutcome {
            task_id:     "test".into(),
            exit_code:   if success { 0 } else { 1 },
            stdout:      String::new(),
            stderr:      if success { String::new() }
                         else { "error[E0425]: cannot find value `foo`".into() },
            duration_ms: 10,
            success,
        })
    }
}

/// In-memory ExperienceStore that records every TestRun stored.
#[derive(Default)]
struct InMemExperience {
    test_runs: Mutex<Vec<TestRun>>,
    memories:  Mutex<Vec<TaskMemory>>,
}
#[async_trait]
impl ExperienceStore for InMemExperience {
    async fn store_task_memory(&self, m: TaskMemory) -> Result<(), ExperienceError> {
        self.memories.lock().unwrap().push(m);
        Ok(())
    }
    async fn recall_insights(&self, _scope: &str) -> Result<Vec<Insight>, ExperienceError> {
        Ok(Vec::new())
    }
    async fn upsert_insight(&self, _i: Insight) -> Result<(), ExperienceError> { Ok(()) }
    async fn recent_task_memories(&self, _l: u64) -> Result<Vec<TaskMemory>, ExperienceError> {
        Ok(Vec::new())
    }
    async fn store_test_run(&self, r: TestRun) -> Result<(), ExperienceError> {
        self.test_runs.lock().unwrap().push(r);
        Ok(())
    }
    async fn test_runs_for_task(&self, task_id: &str) -> Result<Vec<TestRun>, ExperienceError> {
        Ok(self
            .test_runs.lock().unwrap()
            .iter().filter(|r| r.task_id == task_id).cloned().collect())
    }
}

fn cfg_with_workspace() -> ReasoningConfig {
    ReasoningConfig {
        max_steps:           6,
        memory_context_limit: 5,
        system_prompt:       "test".into(),
        verify_retries:      2,
        workspace_path:      Some("/tmp/test-ws".into()),
        ..ReasoningConfig::default()
    }
}

#[tokio::test]
async fn verify_retry_loop_persists_test_runs_and_recovers() {
    // Script: turn 1 emits write_file tool call; turn 2 (synthesis after
    // tool result) emits plain text "done"; turn 3 (after verify failure)
    // emits plain text "fixed".
    let model = MockEngine::new()
        .on("", r#"{"tool":"write_file","args":{"path":"a.rs","content":"fn main(){}"}}"#)
        .on("", "done")
        .on("", "fixed");

    let tools:    Arc<dyn ToolExecutor>  = Arc::new(RecordingExecutor::new());
    let memory:   Arc<dyn MemoryStore>   = Arc::new(InMemoryStore::new());
    let exp_impl                          = Arc::new(InMemExperience::default());
    let exp:      Arc<dyn ExperienceStore> = Arc::clone(&exp_impl)
        as Arc<dyn ExperienceStore>;
    let sandbox:  Arc<dyn Sandbox>       = Arc::new(FlakySandbox::new());

    let loop_ = ReasoningLoop::new(model, tools, memory, cfg_with_workspace())
        .with_experience(exp)
        .with_sandbox(sandbox);

    let result = loop_.run("please write the file").await
        .expect("loop should converge after one retry");

    assert_eq!(result, "fixed", "final text comes from the post-retry turn");

    // Two TestRun rows expected: one failure + one success.
    let runs = exp_impl.test_runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 2, "one TestRun per sandbox invocation");
    assert!(!runs[0].outcome.success, "first run fails");
    assert!(runs[1].outcome.success, "second run passes");
    assert_eq!(runs[0].task_id, runs[1].task_id, "shared task id across attempts");

    // The TaskMemory reflection should run exactly once (one task).
    let memories = exp_impl.memories.lock().unwrap();
    assert_eq!(memories.len(), 1, "one TaskMemory per run() call");
}

#[tokio::test]
async fn verify_skipped_when_no_mutating_tool_call() {
    // Pure-text reply — no write_file / edit_file / bash anywhere.
    let model    = MockEngine::new().on("", "no edits here");
    let tools:    Arc<dyn ToolExecutor>  = Arc::new(RecordingExecutor::new());
    let memory:   Arc<dyn MemoryStore>   = Arc::new(InMemoryStore::new());
    let exp_impl                          = Arc::new(InMemExperience::default());
    let exp:      Arc<dyn ExperienceStore> = Arc::clone(&exp_impl)
        as Arc<dyn ExperienceStore>;
    // Sandbox would fail every time — proves it's never called.
    struct AlwaysFail;
    #[async_trait]
    impl Sandbox for AlwaysFail {
        async fn run(&self, _: ExecutionPlan) -> Result<ExecutionOutcome, SandboxError> {
            panic!("sandbox must not run for non-mutating turns");
        }
    }
    let sandbox: Arc<dyn Sandbox> = Arc::new(AlwaysFail);

    let loop_ = ReasoningLoop::new(model, tools, memory, cfg_with_workspace())
        .with_experience(exp)
        .with_sandbox(sandbox);
    let _ = loop_.run("just chat").await.unwrap();

    assert!(exp_impl.test_runs.lock().unwrap().is_empty(),
            "no TestRun should be stored when no edit happened");
}
