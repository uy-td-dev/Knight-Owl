//! Integration tests for `ReasoningLoop` using `MockEngine` from `owl-harness`.
//!
//! Run with: `cargo test -p owl-brain`

use std::sync::Arc;

use owl_brain::memory::InMemoryStore;
use owl_brain::{BrainError, MemoryStore, ReasoningConfig, ReasoningLoop};
use owl_harness::mock_engine::MockEngine;
use owl_protocol::tools::{ToolCall, ToolResult};

use async_trait::async_trait;
use owl_brain::reasoning_loop::ToolExecutor;

/// A no-op tool executor — always returns "ok".
struct NoOpExecutor;

#[async_trait]
impl ToolExecutor for NoOpExecutor {
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, BrainError> {
        Ok(ToolResult::ok(call.name, serde_json::json!("ok")))
    }
    fn has_tool(&self, _name: &str) -> bool { false }
}

fn cfg() -> ReasoningConfig {
    ReasoningConfig {
        max_steps: 4,
        memory_context_limit: 5,
        system_prompt: "You are a test assistant.".into(),
    }
}

// ── Basic run ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn single_step_returns_response() {
    let model:  MockEngine           = MockEngine::new().on("hello", "Hello from mock!");
    let tools:  Arc<dyn ToolExecutor> = Arc::new(NoOpExecutor);
    let memory: Arc<dyn MemoryStore>  = Arc::new(InMemoryStore::new());

    let loop_ = ReasoningLoop::new(model, tools, memory, cfg());
    let result = loop_.run("hello").await.unwrap();
    assert_eq!(result, "Hello from mock!");
}

#[tokio::test]
async fn memory_stores_user_and_assistant_turns() {
    let model:   MockEngine           = MockEngine::new().on("hi", "Hi back!");
    let tools:   Arc<dyn ToolExecutor> = Arc::new(NoOpExecutor);
    let mem_impl = Arc::new(InMemoryStore::new());
    let mem_ref  = Arc::clone(&mem_impl);
    let memory:  Arc<dyn MemoryStore>  = mem_impl;

    let loop_ = ReasoningLoop::new(model, tools, memory, cfg());
    loop_.run("hi").await.unwrap();

    let entries = mem_ref.recent(10).await.unwrap();
    assert!(entries.len() >= 2, "should have user + assistant entries");
    assert_eq!(entries[0].role, "user");
}

#[tokio::test]
async fn max_steps_exceeded_returns_error() {
    let model:  MockEngine           = MockEngine::new().on("ping", "pong");
    let tools:  Arc<dyn ToolExecutor> = Arc::new(NoOpExecutor);
    let memory: Arc<dyn MemoryStore>  = Arc::new(InMemoryStore::new());

    let loop_ = ReasoningLoop::new(model, tools, memory, ReasoningConfig {
        max_steps: 1,
        ..cfg()
    });
    let result = loop_.run("ping").await;
    assert!(result.is_ok(), "single-step run should succeed: {:?}", result);
}
