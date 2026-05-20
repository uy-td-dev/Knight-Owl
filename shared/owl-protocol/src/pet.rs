//! Virtual pet — the desktop app's Owl chibi companion.
//!
//! Pet state evolves in response to agent activity (tool calls, errors,
//! approvals, idle time).  Persisted in SurrealDB so the pet survives app
//! restarts; one pet per workspace by default.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Mood enum derived from the (happiness, hunger, energy) triple.
///
/// Pure computed property — never persisted; UI re-derives every render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ipc", derive(ts_rs::TS))]
#[cfg_attr(feature = "ipc", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PetMood {
    Happy,     // happiness ≥ 75 and all needs met
    Content,   // baseline
    Hungry,    // hunger ≤ 30
    Tired,     // energy ≤ 20
    Sad,       // happiness ≤ 30
    Sleeping,  // energy ≤ 5 — auto-recovers slowly
    Excited,   // just earned XP / level-up
}

impl PetMood {
    /// Derive the dominant mood from stats.
    ///
    /// Priority: sleeping → tired → hungry → sad → excited (if recent xp event)
    /// → happy → content (default).
    pub fn from_stats(p: &Pet) -> Self {
        if p.energy <= 5         { return Self::Sleeping; }
        if p.energy <= 20        { return Self::Tired; }
        if p.hunger <= 30        { return Self::Hungry; }
        if p.happiness <= 30     { return Self::Sad; }
        if p.happiness >= 75     { return Self::Happy; }
        Self::Content
    }
}

/// Evolutionary form — owl grows with level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ipc", derive(ts_rs::TS))]
#[cfg_attr(feature = "ipc", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PetForm {
    Egg,        // levels 1–2  — hasn't hatched
    Chick,      // levels 3–4  — newborn, helpless
    Owlet,      // levels 5–9  — chibi cute phase
    Adult,      // levels 10–19
    Sage,       // level 20+   — final form
}

impl PetForm {
    pub fn for_level(level: u32) -> Self {
        match level {
            0..=2   => Self::Egg,
            3..=4   => Self::Chick,
            5..=9   => Self::Owlet,
            10..=19 => Self::Adult,
            _       => Self::Sage,
        }
    }
}

/// Persisted pet state.  All numeric fields are clamped 0..=100 by the
/// host before save; consumers can treat them as percentages.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ipc", derive(ts_rs::TS))]
#[cfg_attr(feature = "ipc", ts(export))]
pub struct Pet {
    /// User-visible name.  Defaults to "Hoot" but renameable.
    pub name: String,
    /// Pet level (1+).
    pub level: u32,
    /// Experience toward next level. 100 XP / level.
    pub xp: u32,
    /// 0 = starving, 100 = fed.
    pub hunger: u32,
    /// 0 = depressed, 100 = ecstatic.
    pub happiness: u32,
    /// 0 = asleep, 100 = wired.
    pub energy: u32,
    /// Wall-clock millis (UTC) of the last `feed` action.
    pub last_fed_ms: i64,
    /// Wall-clock millis (UTC) of the last user interaction OR agent event
    /// — used for idle decay computation.
    pub last_active_ms: i64,
    /// Cumulative count of successful tool calls the pet has witnessed.
    pub tools_witnessed: u64,
    /// Optional short cosmetic line — last reaction the pet "said".
    /// Ephemeral; persists so reload doesn't strip the last bubble.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_say: Option<String>,
}

impl Default for Pet {
    fn default() -> Self {
        let now = now_ms();
        Self {
            name:            "Hoot".into(),
            level:           1,
            xp:              0,
            hunger:          70,
            happiness:       80,
            energy:          80,
            last_fed_ms:     now,
            last_active_ms:  now,
            tools_witnessed: 0,
            last_say:        Some("hoot hoot!".into()),
        }
    }
}

impl Pet {
    pub fn form(&self) -> PetForm { PetForm::for_level(self.level) }
    pub fn mood(&self) -> PetMood { PetMood::from_stats(self) }

    /// Apply one [`PetEvent`].  Returns `true` if the pet "leveled up" so
    /// the host can surface a celebration event (confetti, sound, etc.).
    pub fn apply(&mut self, ev: PetEvent) -> bool {
        let now = now_ms();
        self.last_active_ms = now;
        let mut leveled_up = false;
        match ev {
            PetEvent::Fed { amount } => {
                self.hunger = clamp(self.hunger as i64 + amount as i64);
                self.last_fed_ms = now;
                leveled_up |= self.gain_xp(50);
                self.last_say = Some("nom nom".into());
            }
            PetEvent::Played => {
                self.happiness = clamp(self.happiness as i64 + 15);
                self.energy    = clamp(self.energy    as i64 - 10);
                leveled_up |= self.gain_xp(40);
                self.last_say  = Some("wheee!".into());
            }
            PetEvent::Petted => {
                self.happiness = clamp(self.happiness as i64 + 5);
                leveled_up |= self.gain_xp(15);
                self.last_say  = Some("♥".into());
            }
            PetEvent::ToolSuccess => {
                self.tools_witnessed += 1;
                leveled_up |= self.gain_xp(5);
                self.happiness = clamp(self.happiness as i64 + 2);
                self.energy    = clamp(self.energy    as i64 - 1);
                self.last_say  = Some("ooh!".into());
            }
            PetEvent::ToolFailure => {
                self.happiness = clamp(self.happiness as i64 - 3);
                self.last_say  = Some("hmm…".into());
            }
            PetEvent::ApprovalGranted => {
                self.happiness = clamp(self.happiness as i64 + 1);
            }
            PetEvent::ApprovalRejected => {
                self.happiness = clamp(self.happiness as i64 - 2);
            }
            PetEvent::Idle { minutes } => {
                let m = minutes.max(0) as i64;
                self.hunger    = clamp(self.hunger    as i64 - m / 5);
                self.energy    = clamp(self.energy    as i64 - m / 10);
                self.happiness = clamp(self.happiness as i64 - m / 20);
            }
        }
        leveled_up
    }

    /// Add `n` XP, advance level when crossing 100 XP boundaries.
    /// Returns `true` if level changed.
    pub fn gain_xp(&mut self, n: u32) -> bool {
        self.xp += n;
        let mut leveled = false;
        while self.xp >= 100 {
            self.xp -= 100;
            self.level += 1;
            leveled = true;
            // Level-up morale boost.
            self.happiness = clamp(self.happiness as i64 + 10);
        }
        leveled
    }
}

/// Discrete pet-state mutations.  Mapped 1:1 from agent events + user
/// actions in the host layer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ipc", derive(ts_rs::TS))]
#[cfg_attr(feature = "ipc", ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PetEvent {
    /// User clicked "Feed".  `amount` is hunger restored (typically 30).
    Fed             { amount: u32 },
    /// User clicked "Play".
    Played,
    /// User clicked "Pet" (gentle petting).
    Petted,
    /// Agent's tool call succeeded.
    ToolSuccess,
    /// Agent's tool call failed.
    ToolFailure,
    /// User approved a tool-approval prompt.
    ApprovalGranted,
    /// User rejected a tool-approval prompt.
    ApprovalRejected,
    /// Periodic decay tick; `minutes` since last `last_active`.
    Idle            { minutes: i64 },
}

fn clamp(v: i64) -> u32 {
    v.clamp(0, 100) as u32
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pet_starts_happy_owlet_egg() {
        let p = Pet::default();
        assert_eq!(p.level, 1);
        assert_eq!(p.form(), PetForm::Egg);
        assert!(matches!(p.mood(), PetMood::Happy | PetMood::Content));
    }

    #[test]
    fn xp_levels_up_at_100() {
        let mut p = Pet::default();
        p.xp = 95;
        let leveled = p.gain_xp(10);
        assert!(leveled);
        assert_eq!(p.level, 2);
        assert_eq!(p.xp, 5);
    }

    #[test]
    fn tool_success_chain_eventually_levels() {
        let mut p = Pet::default();
        let mut total_levels = 0u32;
        for _ in 0..30 { if p.apply(PetEvent::ToolSuccess) { total_levels += 1; } }
        assert_eq!(total_levels, 1); // 30 × 5 = 150 XP → 1 level + 50 XP
        assert_eq!(p.level, 2);
    }

    #[test]
    fn long_idle_makes_pet_hungry() {
        let mut p = Pet::default();
        p.apply(PetEvent::Idle { minutes: 200 });
        assert!(p.hunger < 40);
        assert_eq!(p.mood(), PetMood::Hungry);
    }

    #[test]
    fn form_transitions_at_milestones() {
        let mut p = Pet::default();
        p.level = 3;  assert_eq!(p.form(), PetForm::Chick);
        p.level = 5;  assert_eq!(p.form(), PetForm::Owlet);
        p.level = 10; assert_eq!(p.form(), PetForm::Adult);
        p.level = 20; assert_eq!(p.form(), PetForm::Sage);
    }
}
