//! Mock tool executor for testing the reasoning loop without real tools.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use owl_brain::reasoning_loop::ToolExecutor;
use owl_brain::BrainError;
use owl_protocol::tools::{ToolCall, ToolResult};

/// Records every tool call and returns scripted results.
pub struct MockToolExecutor {
    /// Scripted responses keyed by tool name.
    scripts: HashMap<String, serde_json::Value>,
    /// Recorded calls in order.
    recorded: Mutex<Vec<ToolCall>>,
}

impl MockToolExecutor {
    /// Create an empty executor (unknown tools return a default result).
    pub fn new() -> Self {
        Self { scripts: HashMap::new(), recorded: Mutex::new(Vec::new()) }
    }

    /// Register a scripted output for a named tool.
    pub fn on(mut self, name: &str, output: serde_json::Value) -> Self {
        self.scripts.insert(name.to_owned(), output);
        self
    }

    /// Return all recorded calls in order.
    pub fn calls(&self) -> Vec<ToolCall> {
        self.recorded.lock().unwrap().clone()
    }
}

impl Default for MockToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for MockToolExecutor {
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, BrainError> {
        self.recorded.lock().unwrap().push(call.clone());
        let output = self
            .scripts
            .get(&call.name)
            .cloned()
            .unwrap_or(serde_json::json!({ "mock": true }));
        Ok(ToolResult::ok(call.name, output))
    }

    fn has_tool(&self, name: &str) -> bool {
        self.scripts.contains_key(name)
    }
}
