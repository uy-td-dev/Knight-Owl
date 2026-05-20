//! Session resume — load the persisted `agent_event` trace for a conv.
//!
//! Frontend pattern: when the user reopens a conversation, call
//! `load_event_log(conv_id)` to fetch every [`AgentEvent`] that happened in
//! prior turns.  Replay them client-side (same handler used for live
//! `agent_event` listener) to reconstruct the full reasoning UI — tool
//! calls, results, state transitions — not just the final messages.

use tauri::State;

use owl_protocol::events::AgentEvent;

use crate::state::AppState;

/// Read every persisted [`AgentEvent`] for `conv_id` in arrival order.
///
/// Returns an empty vec if the log file doesn't exist (new conversation
/// with no prior turns).  Malformed lines are skipped with a warn — never
/// fatal, so a partially-corrupted log can still be replayed up to the bad
/// line.
#[tauri::command]
pub async fn load_event_log(
    conv_id: String,
    state:   State<'_, AppState>,
) -> Result<Vec<AgentEvent>, String> {
    // Sanitise — same rule the sink uses when computing the path.
    let safe: String = conv_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let id = if safe.is_empty() { "default".to_string() } else { safe };
    let path = state.chats_dir.join(format!("{id}.events.jsonl"));

    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.to_string()),
    };

    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() { continue; }
        match serde_json::from_str::<AgentEvent>(line) {
            Ok(ev)  => out.push(ev),
            Err(e)  => tracing::warn!(
                line_no = i + 1, err = %e,
                "load_event_log: skipping malformed line"
            ),
        }
    }
    Ok(out)
}

/// Truncate (delete) the event log for `conv_id`.
///
/// Useful when the user clears a conversation — keep the chat history for
/// reference but drop the verbose tool-call trace to save disk.
#[tauri::command]
pub async fn clear_event_log(
    conv_id: String,
    state:   State<'_, AppState>,
) -> Result<(), String> {
    let safe: String = conv_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let id = if safe.is_empty() { "default".to_string() } else { safe };
    let path = state.chats_dir.join(format!("{id}.events.jsonl"));

    match std::fs::remove_file(&path) {
        Ok(())                                              => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e)                                              => Err(e.to_string()),
    }
}

/// Wipe every distilled `insight` row from the vault.  Useful when the
/// distillation worker has accumulated misleading observations (e.g.
/// "user is learning Rust") that bias agent replies across unrelated
/// conversations.  Invoked from DevTools console:
///
/// ```js
/// await window.__TAURI_INTERNALS__.invoke('clear_insights')
/// ```
#[tauri::command]
pub async fn clear_insights(state: State<'_, AppState>) -> Result<u64, String> {
    let store = match &state.store {
        Some(s) => s.clone(),
        None    => return Err("no SurrealDB connection — nothing to clear".into()),
    };
    store.clear_insights().await.map_err(|e| e.to_string())
}

/// Wipe the conversational memory store.  Use when a small model gets
/// stuck echoing past replies (the recall block dominates its context).
///
/// ```js
/// await window.__TAURI_INTERNALS__.invoke('clear_memory')
/// ```
#[tauri::command]
pub async fn clear_memory(state: State<'_, AppState>) -> Result<u64, String> {
    let store = match &state.store {
        Some(s) => s.clone(),
        None    => return Err("no SurrealDB connection — nothing to clear".into()),
    };
    store.clear_session_memory().await.map_err(|e| e.to_string())
}
