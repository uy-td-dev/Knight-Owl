//! Approval resolution command — frontend → InteractiveGate.

use tauri::State;

use crate::state::AppState;

/// Resolve a pending tool-approval request.
///
/// Called by the UI after the user clicks Approve / Reject in the confirm
/// dialog raised by an `agent_event` of type `tool_awaiting_approval`.
/// Idempotent: a stale or already-resolved `call_id` is silently ignored.
#[tauri::command]
pub async fn resolve_tool_approval(
    call_id:  String,
    approved: bool,
    state:    State<'_, AppState>,
) -> Result<(), String> {
    let mut pending = state.pending_approvals.lock().await;
    if let Some(tx) = pending.remove(&call_id) {
        // Receiver may have been dropped if the agent loop was cancelled —
        // ignore that error (no one is listening anyway).
        let _ = tx.send(approved);
    } else {
        tracing::debug!(call_id, "resolve_tool_approval: no pending request (stale or duplicate)");
    }
    Ok(())
}
