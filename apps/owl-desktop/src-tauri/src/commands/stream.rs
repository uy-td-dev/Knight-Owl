//! Streaming chat command — thin wrapper over `crate::agent::run_agent`.
//!
//! When `input.agent_id` resolves to a registered orchestra agent, we
//! compose its prompt + tool allowlist and pass them as
//! [`crate::agent::AgentOverrides`].  Otherwise the hardcoded default
//! agent runs, exactly as before Phase 5.

use std::collections::HashSet;
use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use owl_orchestra::Registry;
use owl_protocol::orchestra::AgentId;

use crate::agent::{run_agent, AgentOverrides, StreamChunk};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct StreamInput {
    pub id:       String,
    pub message:  String,
    /// Orchestra agent id — `None` keeps the default agent behaviour.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Frontend conversation id — selects which Gemini history to feed.
    /// Each conv has its own wire-format history so switching tabs in the
    /// UI swaps the agent's context (no leakage from older sessions).
    #[serde(default)]
    pub conv_id:  Option<String>,
    /// Multi-modal attachments (images, text files) for vision-capable models.
    #[serde(default)]
    pub attachments: Vec<owl_protocol::attachment::Attachment>,
}

/// Run a user message through the agentic brain and stream chunks back.
#[tauri::command]
pub async fn stream_message(
    input: StreamInput,
    app:   AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Resolve which conversation's history to use.  `None` falls through to
    // a `"default"` slot — keeps backward-compat for any caller that hasn't
    // been updated yet, but normal frontend always sends a real conv_id.
    let conv_id = input.conv_id.unwrap_or_else(|| "default".to_string());

    // Bind the active conv for the event sink so AgentEvents emitted during
    // this turn get appended to `<chats_dir>/<conv_id>.events.jsonl`.
    {
        let mut active = state.active_conv.write().await;
        active.conv_id   = Some(conv_id.clone());
        active.chats_dir = Some(state.chats_dir.clone());
    }

    // ── Provider router ──────────────────────────────────────────────────
    // Legacy custom Gemini HTTP path (`run_agent`) requires a Gemini API key.
    // For Anthropic / Ollama (or any future provider), bypass the legacy path
    // and route through `state.runner` — the brain handles the full reasoning
    // loop and emits `agent_event` Tauri events via [`TauriEventSink`] (wired
    // in `state::build_runner`).  The UI listens to both channels, so the
    // user experience is unified.
    if let Some(api_key) = state.gemini_api_key.clone() {
        // Resolve orchestra-agent overrides if the conversation has one selected.
        let overrides = input.agent_id.as_deref().and_then(|id| resolve_overrides(id, &state));
        let history = state.history_for(&conv_id).await;
        let history_path = state.history_path_for(&conv_id);
        let project_context = crate::commands::project_overview::read_project_context(&state.workspace);

        run_agent(
            input.id,
            input.message,
            app,
            api_key,
            state.model.clone(),
            Arc::clone(&state.tools),
            history,
            history_path,
            overrides,
            project_context,
        ).await;
        return Ok(());
    }

    // Brain-routed path — used for Anthropic, Ollama, and anything else.
    // Pass the user's message UNCHANGED to the runner.  Project context
    // (`<project_stack>` + CLAUDE.md) belongs in the system context, not
    // injected into the user turn — otherwise memory accumulates duplicate
    // blocks and confuses the model with `user: <project_stack>... user: ...`
    // alternations.  ProjectContextProvider (wired in build_runner) handles
    // injection at the right layer.
    let req_id = input.id.clone();
    let runner = Arc::clone(&state.runner);
    let attachments = input.attachments.clone();
    let prompt = input.message.clone();

    tokio::spawn(async move {
        let result = if attachments.is_empty() {
            runner.run(&prompt).await
        } else {
            runner.run_with_attachments(&prompt, &attachments).await
        };
        let (response, error) = match result {
            Ok(text) => (text, None),
            Err(e)   => (String::new(), Some(e.to_string())),
        };
        let _ = app.emit("stream_chunk", StreamChunk {
            id: req_id, text: response,
            thinking: None,
            tool_call: None, tool_args: None, tool_result: None,
            input_tokens: None, output_tokens: None,
            done: true,
            error,
        });
    });

    Ok(())
}

/// Look up an `AgentSpec` in the orchestra registry, resolve its skills,
/// compose the system prompt, and pack into [`AgentOverrides`].
///
/// Returns `None` (with a warning log) if the id is malformed or unknown.
fn resolve_overrides(agent_id: &str, state: &AppState) -> Option<AgentOverrides> {
    let id = match AgentId::new_reserved(agent_id) {
        Ok(v)  => v,
        Err(e) => {
            tracing::warn!(agent_id, %e, "invalid agent_id in stream input");
            return None;
        }
    };
    let spec = match state.orchestra.agent(&id) {
        Some(s) => s,
        None    => {
            tracing::warn!(agent_id, "agent not found in registry; using default");
            return None;
        }
    };
    let (system_prompt, missing) =
        owl_orchestra::compose::compose_for_agent(&spec, &*state.orchestra);
    if !missing.is_empty() {
        tracing::warn!(agent_id, ?missing, "agent references unresolved skills");
    }
    let allowed_tools: HashSet<String> = spec.allowed_tools.iter()
        .map(|t| t.0.clone())
        .collect();
    Some(AgentOverrides { system_prompt, allowed_tools })
}
