//! Native tool listing command.

use owl_armory::registry::build_registry;
use owl_protocol::ipc::{ToolInfo, ToolSource};

/// Return metadata for all registered native tools.
#[tauri::command]
pub fn list_native_tools() -> Vec<ToolInfo> {
    build_registry()
        .iter()
        .map(|t| ToolInfo {
            name:        t.name().to_string(),
            description: t.description().to_string(),
            source:      ToolSource::Native,
        })
        .collect()
}
