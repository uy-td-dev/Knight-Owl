//! Agent state machine types.

/// State of the reasoning loop:
/// `Idle → Planning → Acting → Observing → (Reviewing) → Idle`.
///
/// `Reviewing` is the R-22 review phase that runs *between* the inner loop
/// completion and the final `Idle`.  It is emitted as a state event for UI
/// visibility but is not part of the inner state-machine transition table —
/// the outer verify+review gate manages it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub enum AgentState {
    Idle,
    Planning,
    Acting,
    Observing,
    Reviewing,
}

impl Default for AgentState {
    fn default() -> Self {
        Self::Idle
    }
}
