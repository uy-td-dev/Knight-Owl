//! Approval gate — interactive confirmation before destructive tool calls.
//!
//! The gate is an injectable trait so the brain stays UI-agnostic:
//!
//! - **Headless / CLI / tests** → use [`PermissiveGate`] (auto-approves).
//! - **Desktop UI** → host implements [`ApprovalGate`] backed by a Tauri
//!   event + `oneshot` channel keyed by `call_id`; the UI emits a confirm
//!   dialog on receiving [`AgentEvent::ToolAwaitingApproval`] and resolves
//!   the channel via a `resolve_tool_approval` Tauri command.
//!
//! Per-tool policy is owned by the executor (via [`crate::FilteredExecutor`]
//! when constructed with a policy map); the gate is consulted only when the
//! policy is [`ToolPolicy::RequireApproval`].
//!
//! [`AgentEvent::ToolAwaitingApproval`]: owl_protocol::events::AgentEvent::ToolAwaitingApproval
//! [`ToolPolicy`]: owl_protocol::tools::ToolPolicy

use async_trait::async_trait;

use owl_protocol::tools::ToolCall;

/// Decision returned by an [`ApprovalGate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    Rejected,
}

impl ApprovalDecision {
    pub fn from_bool(b: bool) -> Self {
        if b { Self::Approved } else { Self::Rejected }
    }

    pub fn is_approved(&self) -> bool { matches!(self, Self::Approved) }
}

/// Asks the user whether a pending tool call should run.
///
/// Implementations MUST be cancellation-safe — the caller may drop the
/// future if the agent is interrupted.
#[async_trait]
pub trait ApprovalGate: Send + Sync {
    /// Request approval for `call`.  Optional `reason` is shown to the user.
    ///
    /// The returned `call_id` allows the host to correlate emitted events
    /// with the user's resolution.  Implementations are responsible for
    /// generating a unique id.
    async fn request(&self, call: &ToolCall, reason: Option<&str>) -> ApprovalDecision;
}

/// No-op gate — approves every call.  Suitable for CLI/headless contexts.
pub struct PermissiveGate;

#[async_trait]
impl ApprovalGate for PermissiveGate {
    async fn request(&self, _call: &ToolCall, _reason: Option<&str>) -> ApprovalDecision {
        ApprovalDecision::Approved
    }
}
