//! Phase F — token usage capture from streams → TaskMemory.

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
use owl_protocol::tools::{ToolCall, ToolResult};
use rig::completion::Usage;

struct NoOpExec;
#[async_trait]
impl ToolExecutor for NoOpExec {
    async fn execute(&self, c: ToolCall) -> Result<ToolResult, BrainError> {
        Ok(ToolResult::ok(c.name, serde_json::json!("ok")))
    }
    fn has_tool(&self, _: &str) -> bool { false }
}

#[derive(Default)]
struct RecordingExperience { stored: Mutex<Vec<TaskMemory>> }
#[async_trait]
impl ExperienceStore for RecordingExperience {
    async fn store_task_memory(&self, m: TaskMemory) -> Result<(), ExperienceError> {
        self.stored.lock().unwrap().push(m); Ok(())
    }
    async fn recall_insights(&self, _: &str) -> Result<Vec<Insight>, ExperienceError> { Ok(vec![]) }
    async fn upsert_insight(&self, _: Insight) -> Result<(), ExperienceError> { Ok(()) }
    async fn recent_task_memories(&self, _: u64) -> Result<Vec<TaskMemory>, ExperienceError> {
        Ok(self.stored.lock().unwrap().clone())
    }
    async fn store_test_run(&self, _: TestRun) -> Result<(), ExperienceError> { Ok(()) }
}

#[tokio::test]
async fn token_usage_is_captured_into_task_memory() {
    let usage = Usage {
        input_tokens:                100,
        output_tokens:               42,
        total_tokens:                142,
        cached_input_tokens:         0,
        cache_creation_input_tokens: 0,
    };
    let model = MockEngine::new()
        .on("", "hello back")
        .with_usage(usage);

    let exp_impl = Arc::new(RecordingExperience::default());
    let exp: Arc<dyn ExperienceStore> = Arc::clone(&exp_impl) as Arc<dyn ExperienceStore>;

    let loop_ = ReasoningLoop::new(
        model,
        Arc::new(NoOpExec) as Arc<dyn ToolExecutor>,
        Arc::new(InMemoryStore::new()) as Arc<dyn MemoryStore>,
        ReasoningConfig {
            max_steps:           2,
            memory_context_limit: 5,
            system_prompt:       "test".into(),
            ..ReasoningConfig::default()
        },
    )
    .with_experience(exp);

    loop_.run("hi").await.unwrap();

    let stored = exp_impl.stored.lock().unwrap();
    assert_eq!(stored.len(), 1, "one TaskMemory persisted");
    assert_eq!(stored[0].input_tokens,  100);
    assert_eq!(stored[0].output_tokens, 42);
}

#[tokio::test]
async fn token_usage_zero_when_provider_silent() {
    // No .with_usage() — MockEngine returns None token_usage.
    let model = MockEngine::new().on("", "ok");
    let exp_impl = Arc::new(RecordingExperience::default());
    let exp: Arc<dyn ExperienceStore> = Arc::clone(&exp_impl) as Arc<dyn ExperienceStore>;

    let loop_ = ReasoningLoop::new(
        model,
        Arc::new(NoOpExec) as Arc<dyn ToolExecutor>,
        Arc::new(InMemoryStore::new()) as Arc<dyn MemoryStore>,
        ReasoningConfig {
            max_steps:           2,
            memory_context_limit: 5,
            system_prompt:       "test".into(),
            ..ReasoningConfig::default()
        },
    )
    .with_experience(exp);

    loop_.run("hi").await.unwrap();
    let stored = exp_impl.stored.lock().unwrap();
    assert_eq!(stored[0].input_tokens,  0, "no usage = zero, not panic");
    assert_eq!(stored[0].output_tokens, 0);
}
