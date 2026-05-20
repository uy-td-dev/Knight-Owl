//! Tauri application entry point for Knight-Owl desktop.

pub mod agent;
pub mod agent_factory;
pub mod approval;
pub mod commands;
pub mod event_sink;
pub mod pet_state;
pub mod spawn_agent_tool;
pub mod state;
pub mod tools;

use tauri::Manager;
use tracing_subscriber::EnvFilter;

use state::AppState;

/// Initialize and run the Tauri application.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,owl_desktop=debug,owl_brain=debug,owl_vault=debug")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = AppState::new();
            // Activate the approval gate now that we have a usable AppHandle.
            // Until this assignment runs, InteractiveGate auto-approves
            // (see crate::approval module docs).
            let slot = state.app_handle_slot.clone();
            let handle = app.handle().clone();
            tauri::async_runtime::block_on(async move {
                slot.write().await.replace(handle);
            });
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::approval::resolve_tool_approval,
            commands::session::load_event_log,
            commands::session::clear_event_log,
            commands::session::clear_insights,
            commands::session::clear_memory,
            commands::pet::get_pet,
            commands::pet::feed_pet,
            commands::pet::play_pet,
            commands::pet::pet_pet,
            commands::pet::rename_pet,
            commands::pet::set_pet_level,
            commands::chat::send_message,
            commands::stream::stream_message,
            commands::mcp::list_mcp_servers,
            commands::mcp::add_mcp_server,
            commands::mcp::update_mcp_server,
            commands::mcp::remove_mcp_server,
            commands::tools::list_native_tools,
            commands::workspace::get_workspace,
            commands::workspace::list_workspaces,
            commands::workspace::set_workspace,
            commands::index::index_workspace,
            commands::history::truncate_history,
            commands::conversations::list_conversations,
            commands::conversations::save_conversations,
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::orchestra::list_orchestra_agents,
            commands::orchestra::list_orchestra_skills,
            commands::orchestra::list_orchestra_workflows,
            commands::orchestra::list_orchestra_commands,
            commands::workflow::expand_text_command,
            commands::workflow::run_workflow,
            commands::workflow::cancel_workflow,
            commands::project_overview::get_project_context,
            commands::project_overview::save_project_context,
            commands::knowledge::kb_stats,
            commands::knowledge::kb_files,
            commands::knowledge::kb_file_graph,
            commands::knowledge::kb_node_neighbors,
            commands::knowledge::kb_search,
            commands::knowledge::kb_insights,
            commands::knowledge::kb_entities,
            commands::knowledge::kb_ask,
            commands::knowledge::kb_distill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
