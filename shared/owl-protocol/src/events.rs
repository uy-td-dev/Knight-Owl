//! Cross-crate event types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::state::AgentState;
use crate::tools::{ToolCall, ToolResult};

/// Events emitted by the reasoning loop.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The loop transitioned to a new state.
    StateChanged { state: AgentState },
    /// The LLM produced a text response chunk.
    TextChunk { content: String },
    /// The loop is about to invoke a tool.
    ToolCalling { call: ToolCall },
    /// A tool returned a result.
    ToolCalled { result: ToolResult },
    /// The loop is paused awaiting user approval before invoking `call`.
    ///
    /// `call_id` correlates this event with the user's response sent via
    /// [`AgentEvent::ApprovalResolved`].
    ToolAwaitingApproval {
        call_id: String,
        call: ToolCall,
        /// Optional human-readable rationale (e.g. "writes to disk").
        reason: Option<String>,
    },
    /// User approved or rejected a previously-paused tool call.
    ApprovalResolved { call_id: String, approved: bool },
    /// The loop finished successfully.
    Done { response: String },
    /// The loop encountered an unrecoverable error.
    Error { message: String },
}
