//! Phase E — context compaction.
//!
//! Pre-seeds memory with > `compact_threshold` entries, runs a single
//! task through the loop, asserts compaction fires:
//!   1. Compactor.summarize() is invoked with the oldest slice.
//!   2. MemoryStore.compact() collapses memory to keep_recent + 1
//!      (the summary entry).
//!   3. The summary entry's role is "system" and content prefixed
//!      `[compacted summary]`.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use owl_brain::compactor::Compactor;
use owl_brain::memory::InMemoryStore;
use owl_brain::reasoning_loop::ToolExecutor;
use owl_brain::{BrainError, MemoryStore, ReasoningConfig, ReasoningLoop};
use owl_harness::mock_engine::MockEngine;
use owl_protocol::experience::{
    ExperienceError, ExperienceStore, Insight, TaskMemory, TestRun,
};
use owl_protocol::memory::MemoryEntry;
use owl_protocol::tools::{ToolCall, ToolResult};

struct NoOpExec;
#[async_trait]
impl ToolExecutor for NoOpExec {
    async fn execute(&self, c: ToolCall) -> Result<ToolResult, BrainError> {
        Ok(ToolResult::ok(c.name, serde_json::json!("ok")))
    }
    fn has_tool(&self, _: &str) -> bool { false }
}

#[derive(Default)]
struct InMemExperience { stored: Mutex<Vec<TaskMemory>> }
#[async_trait]
impl ExperienceStore for InMemExperience {
    async fn store_task_memory(&self, m: TaskMemory) -> Result<(), ExperienceError> {
        self.stored.lock().unwrap().push(m); Ok(())
    }
    async fn recall_insights(&self, _: &str) -> Result<Vec<Insight>, ExperienceError> { Ok(vec![]) }
    async fn upsert_insight(&self, _: Insight) -> Result<(), ExperienceError> { Ok(()) }
    async fn recent_task_memories(&self, _: u64) -> Result<Vec<TaskMemory>, ExperienceError> { Ok(vec![]) }
    async fn store_test_run(&self, _: TestRun) -> Result<(), ExperienceError> { Ok(()) }
}

/// Stub compactor — records the slice it was given and emits a fixed
/// summary so the test can assert both invocation and storage shape.
#[derive(Default)]
struct RecordingCompactor {
    invocations: Mutex<Vec<usize>>,
}
#[async_trait]
impl Compactor for RecordingCompactor {
    async fn summarize(&self, entries: &[MemoryEntry]) -> Result<MemoryEntry, BrainError> {
        self.invocations.lock().unwrap().push(entries.len());
        Ok(MemoryEntry {
            role:    "system".into(),
            content: format!("[compacted summary]\n{} entries summarised", entries.len()),
        })
    }
}

fn cfg(threshold: usize, keep_recent: usize) -> ReasoningConfig {
    ReasoningConfig {
        max_steps:           4,
        memory_context_limit: 5,
        system_prompt:       "test".into(),
        compact_threshold:   threshold,
        compact_keep_recent: keep_recent,
        ..ReasoningConfig::default()
    }
}

#[tokio::test]
async fn compactor_shrinks_memory_after_task() {
    let memory: Arc<dyn MemoryStore> = Arc::new(InMemoryStore::new());

    // Seed with 30 entries so the post-task push (user + assistant)
    // pushes total past threshold = 30.
    for i in 0..30 {
        memory.push(MemoryEntry {
            role:    if i % 2 == 0 { "user".into() } else { "assistant".into() },
            content: format!("turn {i}"),
        }).await.unwrap();
    }

    let model    = MockEngine::new().on("", "ok");
    let exp_impl = Arc::new(InMemExperience::default());
    let exp: Arc<dyn ExperienceStore> = Arc::clone(&exp_impl) as Arc<dyn ExperienceStore>;
    let compactor_impl = Arc::new(RecordingCompactor::default());
    let compactor: Arc<dyn Compactor> = Arc::clone(&compactor_impl) as Arc<dyn Compactor>;

    let loop_ = ReasoningLoop::new(
        model,
        Arc::new(NoOpExec) as Arc<dyn ToolExecutor>,
        Arc::clone(&memory),
        cfg(/* threshold = */ 30, /* keep_recent = */ 5),
    )
    .with_experience(exp)
    .with_compactor(compactor);

    loop_.run("hi").await.unwrap();

    // Compactor invoked exactly once.
    let invs = compactor_impl.invocations.lock().unwrap();
    assert_eq!(invs.len(), 1, "compactor called exactly once");

    // Memory should be exactly keep_recent + 1 (summary).
    let after = memory.recent(usize::MAX).await.unwrap();
    assert_eq!(after.len(), 6, "5 kept + 1 summary; got {}", after.len());
    assert_eq!(after[0].role, "system");
    assert!(after[0].content.starts_with("[compacted summary]"));
}

#[tokio::test]
async fn compactor_no_op_when_below_threshold() {
    let memory: Arc<dyn MemoryStore> = Arc::new(InMemoryStore::new());
    for i in 0..5 {
        memory.push(MemoryEntry {
            role: "user".into(),
            content: format!("turn {i}"),
        }).await.unwrap();
    }

    let model = MockEngine::new().on("", "ok");
    let exp:  Arc<dyn ExperienceStore> = Arc::new(InMemExperience::default());
    let compactor_impl = Arc::new(RecordingCompactor::default());
    let compactor: Arc<dyn Compactor> = Arc::clone(&compactor_impl) as Arc<dyn Compactor>;

    let loop_ = ReasoningLoop::new(
        model,
        Arc::new(NoOpExec) as Arc<dyn ToolExecutor>,
        Arc::clone(&memory),
        cfg(50, 5),
    )
    .with_experience(exp)
    .with_compactor(compactor);

    loop_.run("hi").await.unwrap();
    assert!(compactor_impl.invocations.lock().unwrap().is_empty(),
            "compactor not called when below threshold");
}
