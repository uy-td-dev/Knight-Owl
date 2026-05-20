//! Tool input/output types shared across crates.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A request to call a tool by name with JSON arguments.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolCall {
    /// Name of the tool to invoke.
    pub name: String,
    /// JSON-encoded arguments for the tool.
    pub args: serde_json::Value,
}

/// Approval policy for a single tool name.
///
/// Used by [`crate::events::AgentEvent::ToolAwaitingApproval`] flow:
/// the host wraps its [`ToolExecutor`] with a policy + approval gate; tools
/// marked [`ToolPolicy::RequireApproval`] pause the agent and emit an event
/// the UI surfaces as a confirm dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    /// Execute without prompting the user.  Default for read-only tools.
    #[default]
    AutoApprove,
    /// Pause the agent and request user approval before executing.
    /// Recommended for: `bash`, `write_file`, `edit_file`, `apply_patch`,
    /// `multi_edit`, `run_command` (writes), and any MCP tool of unknown safety.
    RequireApproval,
    /// Refuse outright — like the tool isn't in the allowlist.
    Deny,
}

/// The result returned from a tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolResult {
    /// Name of the tool that was invoked.
    pub name: String,
    /// JSON-encoded output from the tool.
    pub output: serde_json::Value,
    /// Whether the invocation succeeded.
    pub success: bool,
    /// Optional error message when `success` is false.
    pub error: Option<String>,
}

impl ToolResult {
    /// Construct a successful result.
    pub fn ok(name: impl Into<String>, output: serde_json::Value) -> Self {
        Self { name: name.into(), output, success: true, error: None }
    }

    /// Construct a failed result.
    pub fn err(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            output: serde_json::Value::Null,
            success: false,
            error: Some(message.into()),
        }
    }
}
