//! Tauri commands surfacing the orchestra workflow engine + slash command
//! expansion to the UI.
//!
//! Two flavours of slash command:
//!
//! - **Text** → synchronous template expansion.  Returned to the frontend
//!   which then drops the result into the chat input box for the user to
//!   review before sending (keeps the user in the loop on prompt rewrites).
//! - **Workflow** → spawns a background task that runs `SequentialEngine`,
//!   forwarding every [`WorkflowEvent`] to the frontend via a Tauri event.
//!   The command itself returns immediately with a `trace_id` the UI uses
//!   to correlate streamed events.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use owl_orchestra::{
    Registry, SequentialEngine, WorkflowEngine, WorkflowEvent, WorkflowInput,
};
use owl_protocol::orchestra::{CommandExpansion, CommandId, TraceId, WorkflowId};

use crate::state::AppState;

// ─── Slash text expansion ────────────────────────────────────────────────────

/// Expand a `Text` command — substitutes `{{$1}}, {{$2}}, …` with the
/// supplied positional args (already split by the frontend).  `args` is
/// 0-indexed, but the placeholders are 1-indexed (matches Hugo / shell
/// convention).
///
/// Errors: unknown id, malformed id, or wrong expansion kind.  All come
/// back as `Err(String)` — frontend toasts them and reverts the input.
#[tauri::command]
pub async fn expand_text_command(
    command_id: String,
    args:       Vec<String>,
    state:      State<'_, AppState>,
) -> Result<String, String> {
    let id = CommandId::new(&command_id).map_err(|e| e.to_string())?;
    let cmd = state.orchestra.command(&id)
        .ok_or_else(|| format!("command `/{}` not found", command_id))?;
    match &cmd.expansion {
        CommandExpansion::Text { template } => {
            // 1-indexed positional args.  Missing args render as empty
            // strings — workflow authors should default in their template
            // (e.g. "{{$1}}" → "(no arg)"), not get cryptic placeholders.
            let mut out = template.clone();
            for (i, arg) in args.iter().enumerate() {
                let placeholder = format!("{{{{${i_one}}}}}", i_one = i + 1);
                out = out.replace(&placeholder, arg);
            }
            // Sweep any remaining `{{$N}}` placeholders that didn't get an
            // arg — replace with empty string rather than leaking the
            // placeholder into the LLM prompt.
            for i in args.len()..16 {
                let placeholder = format!("{{{{${i_one}}}}}", i_one = i + 1);
                out = out.replace(&placeholder, "");
            }
            Ok(out)
        }
        CommandExpansion::Workflow { .. } =>
            Err(format!("`/{}` is a workflow command — call run_workflow", command_id)),
        CommandExpansion::Tool { .. } =>
            Err(format!("`/{}` is a tool command — not supported in v1", command_id)),
    }
}

// ─── Workflow execution ──────────────────────────────────────────────────────

/// Frontend-facing payload for streamed workflow events.
///
/// We re-wrap [`WorkflowEvent`] with a `trace_id` field at the top level
/// so the frontend can filter without parsing the inner enum.  The inner
/// `event` field is the protocol enum verbatim — when harness recordings
/// land in Phase 11 they'll deserialise straight from this.
#[derive(Debug, Serialize, Clone)]
pub struct WorkflowEventPayload {
    pub trace_id: String,
    pub event:    WorkflowEvent,
}

/// Start a workflow.  Returns the freshly-minted `trace_id` synchronously;
/// progress arrives later via the `"workflow_event"` Tauri event.
///
/// The id resolves directly — frontend looks up the workflow when the user
/// picks `/<name>` and passes the workflow's id, not the slash-command id.
/// Unknown workflows return `Err` synchronously.
#[tauri::command]
pub async fn run_workflow(
    workflow_id: String,
    user_input:  String,
    app:         AppHandle,
    state:       State<'_, AppState>,
) -> Result<String, String> {
    // Validate the workflow id and resolve the spec up-front so the user
    // sees errors synchronously instead of via the event stream.
    let wid  = WorkflowId::new(&workflow_id).map_err(|e| e.to_string())?;
    let spec = state.orchestra.workflow(&wid)
        .ok_or_else(|| format!("workflow `{}` not found", workflow_id))?;

    let trace_id = uuid::Uuid::new_v4().to_string();
    let registry = Arc::clone(&state.orchestra) as Arc<dyn Registry>;
    let factory  = Arc::clone(&state.factory);

    // Channel + cancel.  Capacity 256 covers a handful of long workflows
    // without back-pressuring the engine; events are tiny so memory is not
    // a concern.
    let (tx, mut rx) = mpsc::channel::<WorkflowEvent>(256);
    let cancel       = CancellationToken::new();

    // Register the cancel token so `cancel_workflow` can find it later.
    {
        let mut running = state.running_workflows.lock().await;
        running.insert(trace_id.clone(), cancel.clone());
    }

    // Pump task — forwards every event to the webview AND removes the
    // cancel token from the map on terminal events (Completed / Cancelled).
    // Spawned BEFORE the run task so we never miss the initial
    // WorkflowStarted.
    {
        let app      = app.clone();
        let trace_id = trace_id.clone();
        let running  = Arc::clone(&state.running_workflows);
        tokio::spawn(async move {
            while let Some(evt) = rx.recv().await {
                let is_terminal = matches!(
                    evt,
                    WorkflowEvent::WorkflowCompleted { .. }
                    | WorkflowEvent::WorkflowCancelled { .. },
                );
                let payload = WorkflowEventPayload {
                    trace_id: trace_id.clone(),
                    event:    evt,
                };
                if let Err(err) = app.emit("workflow_event", &payload) {
                    tracing::warn!(%err, "workflow_event emit failed");
                }
                if is_terminal {
                    running.lock().await.remove(&trace_id);
                }
            }
            // Channel closed without a terminal event (engine dropped tx
            // without finishing) — clean up defensively.
            running.lock().await.remove(&trace_id);
        });
    }

    // Engine task.  Result is logged but not surfaced — failure is already
    // visible in the event stream as `StepFailed` / `WorkflowCancelled`.
    {
        let trace = TraceId::new(&trace_id);
        let user_input = user_input.clone();
        tokio::spawn(async move {
            let engine = SequentialEngine::new(registry, factory);
            let res = engine.run(
                spec,
                WorkflowInput { trace_id: trace, user_input, seed: None },
                tx, cancel,
            ).await;
            if let Err(e) = res {
                tracing::error!(error = %e, "workflow run terminated with error");
            }
        });
    }

    Ok(trace_id)
}

/// Cancel an in-flight workflow run by `trace_id`.
///
/// Idempotent: cancelling an unknown / already-finished run is a no-op
/// (returns `Ok(false)`).  Successful cancel returns `Ok(true)` and the
/// engine emits `WorkflowCancelled` shortly afterwards via the normal
/// event channel.
#[tauri::command]
pub async fn cancel_workflow(
    trace_id: String,
    state:    State<'_, AppState>,
) -> Result<bool, String> {
    let map = state.running_workflows.lock().await;
    match map.get(&trace_id) {
        Some(token) => {
            token.cancel();
            tracing::info!(trace_id, "workflow cancellation requested");
            Ok(true)
        }
        None => Ok(false),
    }
}
