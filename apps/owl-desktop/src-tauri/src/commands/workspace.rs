//! Workspace management commands.
//!
//! Each workspace is identified by a stable `workspace_id` (`<basename>-<hex>`)
//! derived from its absolute path.  All per-workspace state — SurrealDB
//! database, chat history, agent memory — is keyed by this id, so picking a
//! different folder gives the agent a clean, isolated slate.

use std::path::PathBuf;

use serde::Serialize;
use tauri::State;

use crate::state::{
    read_config, workspace_id_for, write_config, AppConfig, AppState,
};

#[derive(Clone, Serialize)]
pub struct WorkspaceInfo {
    /// Absolute path of the workspace.
    pub path:   String,
    /// Last component of the path — convenient label for the UI.
    pub name:   String,
    /// Stable id used to namespace SurrealDB / chat history.
    pub id:     String,
    /// Whether the path actually exists on disk.
    pub exists: bool,
    /// `true` if this is the workspace currently driving the agent.
    pub active: bool,
}

#[derive(Serialize)]
pub struct WorkspaceList {
    pub active: Option<WorkspaceInfo>,
    pub recent: Vec<WorkspaceInfo>,
}

/// Return the workspace currently used by the agent.
#[tauri::command]
pub fn get_workspace(state: State<'_, AppState>) -> WorkspaceInfo {
    info_for(&state.workspace, true)
}

/// Return the active workspace + every previously-used workspace from config.
#[tauri::command]
pub fn list_workspaces(state: State<'_, AppState>) -> WorkspaceList {
    let cfg = read_config().unwrap_or_default();
    let active_path = state.workspace.display().to_string();
    let active = info_for(&state.workspace, true);

    let recent = cfg.workspaces.iter()
        .filter(|p| **p != active_path)
        .map(|p| info_for(&PathBuf::from(p), false))
        .collect();

    WorkspaceList { active: Some(active), recent }
}

/// Persist a new workspace path and add it to the recent list.
///
/// Takes effect on the next app launch — the agent rebuilds its tool registry
/// and SurrealDB binding scoped to that workspace.
#[tauri::command]
pub fn set_workspace(path: String) -> Result<WorkspaceInfo, String> {
    let pb = PathBuf::from(&path);
    if !pb.is_dir() {
        return Err(format!("not a directory: {path}"));
    }

    let mut cfg = read_config().unwrap_or_default();
    cfg.workspace = Some(path.clone());

    // Move/insert into the recent list (most-recent first, deduped, capped).
    cfg.workspaces.retain(|p| p != &path);
    cfg.workspaces.insert(0, path.clone());
    cfg.workspaces.truncate(20);

    write_config(&cfg).map_err(|e| e.to_string())?;
    Ok(info_for(&pb, false))
}

fn info_for(p: &std::path::Path, active: bool) -> WorkspaceInfo {
    let name = p.file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| p.display().to_string());
    WorkspaceInfo {
        path:   p.display().to_string(),
        name,
        id:     workspace_id_for(p),
        exists: p.is_dir(),
        active,
    }
}
