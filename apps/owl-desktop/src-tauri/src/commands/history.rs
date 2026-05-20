//! Chat-history mutation commands used by the UI's regenerate / edit flows.
//!
//! Histories are per-conversation (Phase: per-conv isolation) — every
//! mutation must therefore name a `conv_id`.  Older callers that omit it
//! fall through to a `"default"` slot to preserve back-compat during
//! upgrades.

use tauri::State;

use crate::agent::sanitize_history;
use crate::state::AppState;

/// Drop everything from a conversation's persisted history at and after
/// `from_index`.
///
/// Counts only `"user"` / `"model"` turns — internal `tool_call` /
/// `tool_response` rows are co-truncated based on their position.  This lets
/// the UI tell us "delete the last assistant reply" (`from_index = N-1`) or
/// "delete from this user message onward" (`from_index = N`) by passing the
/// visible-message index without needing to know about the wire format.
#[tauri::command]
pub async fn truncate_history(
    from_index: usize,
    #[allow(non_snake_case)]
    conv_id:    Option<String>,
    state:      State<'_, AppState>,
) -> Result<usize, String> {
    let cid = conv_id.unwrap_or_else(|| "default".to_string());
    let history = state.history_for(&cid).await;
    let mut h = history.lock().await;

    // Find the raw position corresponding to the `from_index`-th visible turn.
    let mut visible_seen = 0usize;
    let mut cut_at      = h.len();
    for (i, turn) in h.iter().enumerate() {
        if matches!(turn.role.as_str(), "user" | "model") {
            if visible_seen == from_index {
                cut_at = i;
                break;
            }
            visible_seen += 1;
        }
    }

    h.truncate(cut_at);

    // After truncation the tail may be a lone `tool_call` (its matching
    // response was after the cut) or the head may now begin with an orphan
    // `tool_response`.  Either makes the next Gemini request 400.
    sanitize_history(&mut h);

    // Persist immediately so a crash before the next turn doesn't restore.
    if let Ok(json) = serde_json::to_string_pretty(&*h) {
        let _ = std::fs::write(&state.history_path_for(&cid), json);
    }

    Ok(cut_at)
}
