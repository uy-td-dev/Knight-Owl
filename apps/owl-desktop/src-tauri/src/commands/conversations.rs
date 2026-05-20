//! Persistence for the UI conversation list (sidebar threads).
//!
//! These commands round-trip the entire `Conversation[]` JSON the frontend
//! manages — the backend stores it as opaque text so we don't have to mirror
//! every TypeScript field in Rust.
//!
//! Storage location:
//!   • `<workspace>/.knight-owl/conversations.json` when the user has picked
//!     a workspace (env var `OWL_WORKSPACE` or `AppConfig::workspace` set).
//!   • `~/.knight-owl/conversations.json` otherwise — the "no workspace" /
//!     ad-hoc CWD case.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::state::{read_config, AppState};

/// Resolve the JSON path for the active workspace; falls back to the global
/// `~/.knight-owl/` directory when no workspace was explicitly picked.
fn conversations_path(workspace: &Path) -> PathBuf {
    let user_picked =
        std::env::var("OWL_WORKSPACE").is_ok()
        || read_config().and_then(|c| c.workspace).is_some();

    if user_picked && workspace.is_dir() {
        return workspace.join(".knight-owl").join("conversations.json");
    }

    if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".knight-owl");
        p.push("conversations.json");
        return p;
    }
    PathBuf::from(".knight-owl/conversations.json")
}

/// Read the persisted conversation list as raw JSON.
///
/// Returns `"[]"` when the file does not yet exist so the frontend can treat
/// the response uniformly.
#[tauri::command]
pub async fn list_conversations(state: State<'_, AppState>) -> Result<String, String> {
    let path = conversations_path(&state.workspace);
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            // Validate it parses as JSON; if not, treat as empty so a corrupt
            // file doesn't break the UI on launch.
            if serde_json::from_str::<serde_json::Value>(&s).is_ok() {
                Ok(s)
            } else {
                tracing::warn!(path = %path.display(), "conversations.json is not valid JSON");
                Ok("[]".into())
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("[]".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Persist the conversation list.  Frontend sends a pre-serialized JSON string
/// of `Conversation[]`; we just validate + write.
#[tauri::command]
pub async fn save_conversations(json: String, state: State<'_, AppState>) -> Result<(), String> {
    serde_json::from_str::<serde_json::Value>(&json)
        .map_err(|e| format!("invalid JSON payload: {e}"))?;

    let path = conversations_path(&state.workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, &json).map_err(|e| e.to_string())?;
    tracing::debug!(path = %path.display(), bytes = json.len(), "saved conversations");
    Ok(())
}
