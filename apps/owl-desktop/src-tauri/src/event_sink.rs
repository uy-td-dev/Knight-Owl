//! Tauri-backed [`EventSink`] — bridges brain events to the React frontend
//! AND persists them to per-conversation event logs for resume.
//!
//! ## Two responsibilities
//!
//! 1. **Live emit** — every [`AgentEvent`] is forwarded on the
//!    `"agent_event"` Tauri channel; frontend `listen('agent_event', ...)`
//!    renders step-level progress (state, tool calls, results, approval).
//!
//! 2. **Replay log** — when an `active_conv_id` is set (per turn, by
//!    `commands::stream`), every event is also appended as JSONL to
//!    `<chats_dir>/<conv_id>.events.jsonl`.  On resume the frontend can read
//!    this file and re-render the full reasoning trace, not just the final
//!    text — true session resume, not just message restoration.
//!
//! Late-bound `AppHandle` (via [`AppHandleSlot`]) so the sink can be
//! constructed inside `build_runner()` and activated post-`setup()`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tauri::Emitter;
use tokio::sync::RwLock;

use owl_brain::EventSink;
use owl_protocol::events::AgentEvent;

use crate::approval::AppHandleSlot;

/// Per-turn binding: which conversation id is the active sink writing to,
/// and where on disk are its event logs?  Set by `commands::stream` before
/// each turn so events land in the right `*.events.jsonl` file.
#[derive(Clone, Default)]
pub struct ActiveConv {
    pub conv_id:   Option<String>,
    pub chats_dir: Option<PathBuf>,
}

pub type ActiveConvSlot = Arc<RwLock<ActiveConv>>;

/// Tauri event sink.  Forwards every [`AgentEvent`] to the `"agent_event"`
/// channel, appends it to the per-conv event log for replay, AND feeds the
/// Owl chibi pet so it reacts to tool calls / approvals in real time.
pub struct TauriEventSink {
    app_slot:    AppHandleSlot,
    active_conv: ActiveConvSlot,
    pet:         Option<crate::pet_state::PetState>,
}

impl TauriEventSink {
    pub fn new(
        app_slot: AppHandleSlot,
        active_conv: ActiveConvSlot,
        pet: Option<crate::pet_state::PetState>,
    ) -> Self {
        Self { app_slot, active_conv, pet }
    }

    /// On-disk path for the event log of `conv_id` under `chats_dir`.
    fn event_log_path(chats_dir: &std::path::Path, conv_id: &str) -> PathBuf {
        // Use the same id-sanitisation rule as `AppState::history_path_for`.
        let safe: String = conv_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let id = if safe.is_empty() { "default".to_string() } else { safe };
        chats_dir.join(format!("{id}.events.jsonl"))
    }

    /// Append `event` as a single JSONL line to the active conv's event log.
    /// Best-effort — logging failures are warned but never propagated.
    async fn persist(&self, event: &AgentEvent) {
        let active = self.active_conv.read().await.clone();
        let (Some(conv_id), Some(chats_dir)) = (active.conv_id, active.chats_dir) else {
            return; // no active binding — caller didn't set one (e.g. CLI / sub-agent path)
        };
        let path = Self::event_log_path(&chats_dir, &conv_id);
        let line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(err = %e, "event_sink: serialize failed");
                return;
            }
        };
        // Spawn the disk write so we never block the loop on filesystem latency.
        tokio::task::spawn_blocking(move || {
            use std::fs::OpenOptions;
            use std::io::Write;
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(mut f) => {
                    if let Err(e) = writeln!(f, "{line}") {
                        tracing::warn!(err = %e, path = %path.display(), "event log append failed");
                    }
                }
                Err(e) => tracing::warn!(err = %e, path = %path.display(), "event log open failed"),
            }
        });
    }
}

#[async_trait]
impl EventSink for TauriEventSink {
    async fn emit(&self, event: AgentEvent) {
        // Persist to log first so a Tauri emit failure doesn't lose the trace.
        self.persist(&event).await;

        // Feed the pet — fire-and-forget so the agent loop isn't blocked.
        if let Some(pet) = &self.pet {
            pet.apply_agent_event(&event).await;
        }

        let app = match self.app_slot.read().await.clone() {
            Some(h) => h,
            None    => return, // setup() hasn't populated the handle yet
        };
        if let Err(e) = app.emit("agent_event", &event) {
            tracing::warn!(err = %e, "TauriEventSink: emit failed");
        }
    }
}
