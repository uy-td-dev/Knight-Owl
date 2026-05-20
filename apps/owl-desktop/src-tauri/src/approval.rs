//! Interactive approval gate backed by Tauri events + oneshot channels.
//!
//! Wires the brain's [`ApprovalGate`] trait to the desktop UI:
//!
//! 1. Brain calls `gate.request(call)` for any tool with policy
//!    [`ToolPolicy::RequireApproval`].
//! 2. The gate generates a UUID `call_id`, parks a `oneshot::Sender<bool>`
//!    in the pending-map, and emits an `agent_event` with
//!    [`AgentEvent::ToolAwaitingApproval`].
//! 3. The frontend receives the event, shows a confirm dialog, and invokes
//!    the `resolve_tool_approval` Tauri command (see `commands/approval.rs`).
//! 4. That command looks up the parked sender by `call_id` and forwards the
//!    user's decision; the gate's `await` resolves and the brain proceeds.
//!
//! Failure modes:
//! - If the UI crashes or the user closes the app without responding, the
//!   sender is dropped and `await` returns [`ApprovalDecision::Rejected`]
//!   (fail-closed default — never run an unconfirmed tool).
//! - If the same `call_id` is resolved twice, the second resolution is a
//!   no-op (sender already consumed).
//!
//! [`ApprovalGate`]: owl_brain::ApprovalGate
//! [`ToolPolicy`]: owl_protocol::tools::ToolPolicy
//! [`AgentEvent::ToolAwaitingApproval`]: owl_protocol::events::AgentEvent::ToolAwaitingApproval

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, Mutex, RwLock};

use owl_brain::{ApprovalDecision, ApprovalGate};
use owl_protocol::events::AgentEvent;
use owl_protocol::tools::ToolCall;

/// Map of pending approval requests, keyed by `call_id`.
pub type PendingApprovals = Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>;

/// Late-bound `AppHandle` slot — set by `setup()` after the Tauri builder
/// produces the handle.  Allows the gate to be constructed inside
/// `build_runner()` (before `AppHandle` exists) and activated post-setup.
pub type AppHandleSlot = Arc<RwLock<Option<AppHandle>>>;

/// Approval gate that emits a Tauri event and waits for a UI response.
///
/// Until [`AppHandleSlot`] is populated by `setup()`, every request
/// auto-approves (so early startup tool calls — e.g. boot-time MCP probes
/// — don't deadlock waiting for a UI that hasn't mounted yet).  Once the
/// handle is set, full interactive approval is enforced.
pub struct InteractiveGate {
    app_slot: AppHandleSlot,
    pending:  PendingApprovals,
}

impl InteractiveGate {
    pub fn new(app_slot: AppHandleSlot, pending: PendingApprovals) -> Self {
        Self { app_slot, pending }
    }
}

#[async_trait]
impl ApprovalGate for InteractiveGate {
    async fn request(&self, call: &ToolCall, reason: Option<&str>) -> ApprovalDecision {
        // Snapshot the AppHandle while holding the read lock as briefly as possible.
        let app = match self.app_slot.read().await.clone() {
            Some(h) => h,
            None    => {
                tracing::debug!(tool = %call.name, "approval gate inactive; auto-approving");
                return ApprovalDecision::Approved;
            }
        };

        let call_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel::<bool>();

        // Park the sender BEFORE emitting so the UI can't beat us to resolve.
        self.pending.lock().await.insert(call_id.clone(), tx);

        let event = AgentEvent::ToolAwaitingApproval {
            call_id: call_id.clone(),
            call:    call.clone(),
            reason:  reason.map(str::to_string),
        };
        if let Err(e) = app.emit("agent_event", &event) {
            tracing::warn!(err = %e, "failed to emit ToolAwaitingApproval");
            self.pending.lock().await.remove(&call_id);
            return ApprovalDecision::Rejected;
        }

        // Await the user's response.  Sender drop → recv() returns Err → fail closed.
        match rx.await {
            Ok(approved) => ApprovalDecision::from_bool(approved),
            Err(_)       => {
                tracing::warn!(call_id, "approval channel dropped; defaulting to rejected");
                ApprovalDecision::Rejected
            }
        }
    }
}
