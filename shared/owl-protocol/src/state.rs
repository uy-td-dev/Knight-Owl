//! Agent state machine types.

/// State of the reasoning loop: `Idle → Planning → Acting → Observing → Idle`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub enum AgentState {
    Idle,
    Planning,
    Acting,
    Observing,
}

impl Default for AgentState {
    fn default() -> Self {
        Self::Idle
    }
}
