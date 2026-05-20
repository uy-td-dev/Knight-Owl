//! MCP server configuration management commands.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use owl_protocol::ipc::McpServerConfig;

fn config_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("mcp_servers.json")
}

fn load(app: &AppHandle) -> Result<Vec<McpServerConfig>, String> {
    let path = config_path(app);
    if !path.exists() { return Ok(vec![]); }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

fn save(app: &AppHandle, servers: &[McpServerConfig]) -> Result<(), String> {
    let path = config_path(app);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(servers).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// Return all configured MCP servers.
#[tauri::command]
pub fn list_mcp_servers(app: AppHandle) -> Result<Vec<McpServerConfig>, String> {
    load(&app)
}

/// Persist a new MCP server entry (id must be a fresh UUID).
#[tauri::command]
pub fn add_mcp_server(app: AppHandle, config: McpServerConfig) -> Result<(), String> {
    let mut servers = load(&app)?;
    if servers.iter().any(|s| s.id == config.id) {
        return Err(format!("server '{}' already exists", config.id));
    }
    servers.push(config);
    save(&app, &servers)
}

/// Update an existing MCP server by id.
#[tauri::command]
pub fn update_mcp_server(app: AppHandle, config: McpServerConfig) -> Result<(), String> {
    let mut servers = load(&app)?;
    if let Some(s) = servers.iter_mut().find(|s| s.id == config.id) {
        *s = config;
        save(&app, &servers)
    } else {
        Err(format!("server '{}' not found", config.id))
    }
}

/// Delete an MCP server by id.
#[tauri::command]
pub fn remove_mcp_server(app: AppHandle, id: String) -> Result<(), String> {
    let mut servers = load(&app)?;
    let before = servers.len();
    servers.retain(|s| s.id != id);
    if servers.len() == before {
        return Err(format!("server '{id}' not found"));
    }
    save(&app, &servers)
}
