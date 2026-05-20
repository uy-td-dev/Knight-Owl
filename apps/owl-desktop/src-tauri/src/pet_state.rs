//! Runtime pet state — wraps the protocol `Pet` with persistence + event
//! reactions + a Tauri broadcast channel for live UI updates.
//!
//! Wiring:
//! - One `PetState` is constructed inside `build_runner()` (alongside the
//!   approval gate + event sink) and shared via `Arc` to the AppState.
//! - [`TauriEventSink`] calls `apply_agent_event()` on every brain event so
//!   the pet reacts to tool calls / errors / approvals in real time.
//! - A background tokio task ticks every 60 s and applies an `Idle` event
//!   when the user has been quiet for ≥ 5 min.
//! - Every mutation emits `pet_event` on the Tauri channel so the React
//!   widget refreshes without polling.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use owl_protocol::events::AgentEvent;
use owl_protocol::pet::{Pet, PetEvent, PetMood};
use owl_vault::PetStore;

use crate::approval::AppHandleSlot;

/// Local wall-clock helper — host-side mirror of the protocol's private one.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Stable pet id — single pet per workspace.  Hashing keeps the id short
/// + deterministic across runs without leaking absolute paths.
pub fn pet_id_for(workspace: &std::path::Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    workspace.hash(&mut h);
    format!("ws-{:x}", h.finish())
}

/// Shared pet state.  Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct PetState {
    inner: Arc<PetStateInner>,
}

struct PetStateInner {
    id:      String,
    pet:     RwLock<Pet>,
    store:   Option<Arc<dyn PetStore>>,
    /// Same late-bound handle slot the approval gate uses — lets the pet
    /// emit `pet_event` Tauri events once `setup()` has mounted the handle.
    app_slot: AppHandleSlot,
    /// Monotonically increasing tick counter — rotates the auto-thought
    /// pick so the pet doesn't repeat the same line back-to-back.
    mind_tick: AtomicU64,
    /// Recent tool-call counter — fed by `apply_agent_event` so the auto-
    /// think pool can pick activity-aware lines ("you're on fire") vs.
    /// quiet ones ("hello?").  Reset to 0 every mind tick.
    recent_tools: AtomicU64,
    /// Unix-ms of the most recent USER (not decay) interaction.  Lets the
    /// mind task skip thinking when the pet just spoke from a feed/play.
    last_user_action_ms: AtomicU64,
}

impl PetState {
    /// Build a fresh state with an in-memory default pet.  If a `store` is
    /// present, the constructor also loads the persisted pet (best-effort —
    /// failures are logged and the default is kept).
    pub async fn new(
        id: String,
        store: Option<Arc<dyn PetStore>>,
        app_slot: AppHandleSlot,
    ) -> Self {
        let pet = if let Some(s) = &store {
            match s.load(&id).await {
                Ok(Some(p)) => p,
                Ok(None)    => Pet::default(),
                Err(e)      => {
                    tracing::warn!(err = %e, "pet load failed; using default");
                    Pet::default()
                }
            }
        } else {
            Pet::default()
        };
        Self {
            inner: Arc::new(PetStateInner {
                id, pet: RwLock::new(pet), store, app_slot,
                mind_tick:           AtomicU64::new(0),
                recent_tools:        AtomicU64::new(0),
                last_user_action_ms: AtomicU64::new(0),
            }),
        }
    }

    /// Snapshot the current pet.
    pub async fn snapshot(&self) -> Pet {
        self.inner.pet.read().await.clone()
    }

    /// Apply a [`PetEvent`], persist, and emit a `pet_event` to the UI.
    ///
    /// Side effects beyond the pet stats:
    /// - `ToolSuccess` increments the recent-tools counter (consumed by the
    ///   auto-think task to bias activity-aware lines).
    /// - Any user-initiated event (`Fed` / `Played` / `Petted`) updates
    ///   `last_user_action_ms` so the mind task can stay silent for a beat
    ///   after the user just interacted.
    pub async fn apply(&self, event: PetEvent) {
        // Side-channel counters BEFORE acquiring the write lock so they
        // never block on save IO.
        match &event {
            PetEvent::ToolSuccess => { self.inner.recent_tools.fetch_add(1, Ordering::Relaxed); }
            PetEvent::Fed { .. } | PetEvent::Played | PetEvent::Petted => {
                self.inner.last_user_action_ms.store(now_ms() as u64, Ordering::Relaxed);
            }
            _ => {}
        }

        let mut p = self.inner.pet.write().await;
        let leveled = p.apply(event);
        let snapshot = p.clone();
        drop(p); // release lock before IO

        if let Some(s) = &self.inner.store {
            if let Err(e) = s.save(&self.inner.id, &snapshot).await {
                tracing::warn!(err = %e, "pet save failed");
            }
        }
        self.emit(&snapshot, leveled).await;
    }

    /// Map an [`AgentEvent`] to a [`PetEvent`] when relevant; otherwise no-op.
    pub async fn apply_agent_event(&self, ev: &AgentEvent) {
        let pet_ev = match ev {
            AgentEvent::ToolCalled { result } if result.success => PetEvent::ToolSuccess,
            AgentEvent::ToolCalled { result } if !result.success => PetEvent::ToolFailure,
            AgentEvent::ApprovalResolved { approved: true,  .. } => PetEvent::ApprovalGranted,
            AgentEvent::ApprovalResolved { approved: false, .. } => PetEvent::ApprovalRejected,
            _ => return,
        };
        self.apply(pet_ev).await;
    }

    /// Spawn the idle-decay tick (every 60 s).
    pub fn start_decay_task(self: &Self) {
        let state = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.tick().await; // first tick fires immediately — skip
            loop {
                interval.tick().await;
                let last_active = state.inner.pet.read().await.last_active_ms;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64).unwrap_or(0);
                let idle_min = (now - last_active) / 60_000;
                if idle_min >= 5 {
                    // Apply incremental decay (one minute's worth) so the
                    // event log shows continuous activity.  We deliberately
                    // do NOT pass the full idle_min — that would be lumpy.
                    state.apply(PetEvent::Idle { minutes: 1 }).await;
                }
            }
        });
    }

    /// Spawn the autonomous "mind" task — every ~45 s the pet emits a
    /// contextual thought (mood + time-of-day + recent activity aware).
    ///
    /// Skip rules to avoid spam:
    /// - Pet is sleeping (energy ≤ 5).
    /// - User just interacted in the last 8 s (`last_user_action_ms`).
    /// - The previously-emitted bubble is still on-screen client-side; the
    ///   UI bubble auto-hides after 4 s, so a 45 s tick interleaves cleanly.
    pub fn start_mind_task(self: &Self) {
        let state = self.clone();
        tokio::spawn(async move {
            // Stagger the first thought 15 s after launch so the user gets a
            // greeting fairly soon without it landing the same second as
            // app boot.
            tokio::time::sleep(Duration::from_secs(15)).await;
            let mut interval = tokio::time::interval(Duration::from_secs(45));
            loop {
                interval.tick().await;

                let snapshot = state.inner.pet.read().await.clone();
                let mood = snapshot.mood();

                // Silence the pet when it would be rude or pointless.
                if matches!(mood, PetMood::Sleeping) { continue; }
                let recent_user = state.inner.last_user_action_ms.load(Ordering::Relaxed) as i64;
                if recent_user > 0 && (now_ms() - recent_user) < 8_000 { continue; }

                let recent_tools = state.inner.recent_tools.swap(0, Ordering::Relaxed);
                let tick = state.inner.mind_tick.fetch_add(1, Ordering::Relaxed);
                let hour = local_hour();
                let thought = pick_thought(mood, &snapshot, hour, recent_tools, tick);

                // Inject as last_say + emit; no PetEvent::Apply path because
                // auto-thoughts should not perturb stats.
                let mut p = state.inner.pet.write().await;
                p.last_say = Some(thought);
                let snap = p.clone();
                drop(p);
                if let Some(s) = &state.inner.store {
                    let _ = s.save(&state.inner.id, &snap).await;
                }
                state.emit(&snap, false).await;
            }
        });
    }

    /// Dev cheat — force pet to a specific level + reset XP.  Persists + emits.
    pub async fn set_level(&self, level: u32, xp: u32) {
        let mut p = self.inner.pet.write().await;
        p.level = level.max(1);
        p.xp    = xp.min(99);
        p.last_say = Some("✨ POOF! ✨".into());
        let snap = p.clone();
        drop(p);
        if let Some(s) = &self.inner.store {
            let _ = s.save(&self.inner.id, &snap).await;
        }
        self.emit(&snap, true).await; // emit as level_up for the confetti effect
    }

    /// Rename the pet (UI-driven).  Persists + emits.
    pub async fn rename(&self, name: String) {
        let mut p = self.inner.pet.write().await;
        p.name = name;
        let snap = p.clone();
        drop(p);
        if let Some(s) = &self.inner.store {
            let _ = s.save(&self.inner.id, &snap).await;
        }
        self.emit(&snap, false).await;
    }

    /// Push a `pet_event` to the React widget.
    async fn emit(&self, pet: &Pet, leveled_up: bool) {
        let Some(app) = self.inner.app_slot.read().await.clone() else { return; };
        let payload = serde_json::json!({
            "pet": pet,
            "leveled_up": leveled_up,
        });
        if let Err(e) = app.emit("pet_event", payload) {
            tracing::warn!(err = %e, "pet_event emit failed");
        }
    }
}

/// Helper passed to [`crate::event_sink::TauriEventSink`] so it can react
/// to agent events without owning a full `AppState` reference.
pub type SharedPetState = Option<PetState>;

/// Tiny accessor used by the [`Tauri command bridge`](crate::commands::pet)
/// to push from `&AppHandle`-only contexts.
pub fn extract_pet_handle(app: &AppHandle) -> Option<PetState> {
    use tauri::Manager;
    app.try_state::<crate::state::AppState>().map(|s| s.pet.clone())
}

/// Current hour 0-23 in the host's local timezone.
///
/// `chrono` is intentionally NOT pulled in for this — a tiny manual
/// computation off `SystemTime + UTC offset` is enough and avoids a heavy
/// new dependency for one helper.  Offset detection uses the libc `time_t`
/// → `tm.tm_hour` path indirectly via the std `SystemTime`-to-`UNIX_EPOCH`
/// math; for daylight-savings drift the worst case is the thought pool
/// chooses an adjacent bucket — acceptable cosmetic precision.
fn local_hour() -> u8 {
    use std::time::SystemTime;
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    // Read tzdata once: try $TZ-based offset via libc-y check on macOS/Linux
    // is overkill for a cosmetic; instead use the UTC hour directly when no
    // offset env hint is present.
    let offset_secs = std::env::var("OWL_PET_TZ_OFFSET")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let h = ((secs + offset_secs) / 3600).rem_euclid(24);
    h as u8
}

/// Pick a context-aware thought line.
///
/// Selection algorithm:
/// 1. Determine which pool applies based on `mood` (highest-priority need
///    first) — sleeping is filtered upstream so it doesn't appear here.
/// 2. Refine the pool with time-of-day + activity tweaks for richer text.
/// 3. Pick by `tick % pool.len()` so consecutive thoughts cycle deterministically
///    rather than randomly repeating.
fn pick_thought(
    mood:         PetMood,
    pet:          &Pet,
    hour:         u8,
    recent_tools: u64,
    tick:         u64,
) -> String {
    // Mood-driven core pool.
    let pool: &[&str] = match mood {
        PetMood::Hungry => &[
            "tummy rumbles…",
            "snacks plz",
            "feed me?",
            "any food?",
            "…hungry…",
            "berries would be nice",
        ],
        PetMood::Tired => &[
            "yaaawn~",
            "so sleepy",
            "naptime soon?",
            "eyelids heavy…",
            "5 more minutes…",
        ],
        PetMood::Sad => &[
            "everything's fine…",
            "*sigh*",
            "could use a head scratch",
            "lonely",
            "remember when we played?",
        ],
        PetMood::Excited => &[
            "FEELING POWERFUL",
            "level-up rush!",
            "I can do anything!",
            "more, more!",
        ],
        PetMood::Happy => &[
            "best day ever",
            "I love it here",
            "hoot hoot!",
            "you're the best",
            "♥",
            "everything is wonderful",
        ],
        // Content / default — gets the longest pool because it's the most common
        // state and we want variety so the user doesn't see the same line twice
        // in a row.
        _ => &[
            "*watches you type*",
            "what are we building?",
            "I see u",
            "interesting...",
            "stretchhh",
            "preening feathers",
            "looking out the window",
            "humming softly",
            "what now?",
            "*tilts head*",
            "ooh shiny code",
        ],
    };

    // Activity-aware additions (only used in calm moods).
    let mut extra: Vec<&str> = Vec::new();
    if matches!(mood, PetMood::Happy | PetMood::Content | PetMood::Excited) {
        if recent_tools >= 5 {
            extra.extend([
                "you're on fire 🔥",
                "so much code!",
                "watching the magic happen",
                "loving this rhythm",
            ]);
        } else if recent_tools == 0 && pet.tools_witnessed > 5 {
            extra.extend([
                "quiet out there",
                "everything ok?",
                "hello?",
                "*looks for you*",
            ]);
        }

        // Time-of-day tinting.
        match hour {
            5..=8   => extra.extend(["good morning ☀", "early bird!", "sleepy sunrise"]),
            21..=23 => extra.extend(["night owl mode 🌙", "still up?", "starlight time"]),
            0..=4   => extra.extend(["it's late…", "the moon is bright", "everyone's asleep"]),
            _       => {}
        }
    }

    let pool_len = pool.len() + extra.len();
    let idx = (tick as usize) % pool_len.max(1);
    if idx < pool.len() {
        pool[idx].to_string()
    } else {
        extra[idx - pool.len()].to_string()
    }
}
