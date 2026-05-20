//! Chat command — thin wrapper over the reasoning loop.

use tauri::State;

use owl_protocol::ipc::{ChatInput, ChatOutput};

use crate::state::AppState;

/// Send a user message to the agent and receive a response.
#[tauri::command]
pub async fn send_message(
    msg: ChatInput,
    state: State<'_, AppState>,
) -> Result<ChatOutput, String> {
    let response = state.runner.run(&msg.message).await.map_err(|e| e.to_string())?;
    let session_id = msg.session_id.unwrap_or_else(session_id_new);
    Ok(ChatOutput { response, session_id })
}

fn session_id_new() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("session-{nanos}")
}
