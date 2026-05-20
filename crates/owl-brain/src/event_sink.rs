//! Event sink — pluggable observer for [`AgentEvent`] emitted by the loop.
//!
//! Lets the host (CLI, desktop, tests) subscribe to step-level reasoning
//! progress without coupling owl-brain to any specific transport.
//!
//! ## What you get today (step-level streaming)
//!
//! Inside the loop, an event is emitted for every meaningful boundary:
//!
//! - [`AgentEvent::StateChanged`] — every state-machine transition.
//! - [`AgentEvent::ToolCalling`] — before each tool dispatch (one per
//!   parallel call in a turn).
//! - [`AgentEvent::ToolCalled`] — after each tool result.
//! - [`AgentEvent::TextChunk`]   — once per assistant response (whole
//!   message, since `rig 0.9` returns the full string from `prompt().await`).
//! - [`AgentEvent::Done`]        — final answer.
//! - [`AgentEvent::Error`]       — unrecoverable failure.
//!
//! ## What's NOT here yet (token-level streaming)
//!
//! `rig 0.9` does not expose a streaming completion API; the assistant text
//! arrives as a single `String`.  When the workspace bumps to a rig version
//! with `StreamingPrompt`, the loop swaps `agent.prompt(...).await` for a
//! token stream and emits one `TextChunk` per delta — no other consumer
//! changes are required.
//!
//! ## Default behaviour
//!
//! [`NullSink`] is the no-op default; existing callers that don't care about
//! events keep working unchanged.

use async_trait::async_trait;

use owl_protocol::events::AgentEvent;

/// Pluggable observer for events emitted by the reasoning loop.
///
/// Implementations MUST be cheap and non-blocking — the loop awaits each
/// `emit()` inline.  Heavy work (file IO, network) belongs in a background
/// task spawned from inside the sink.
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&self, event: AgentEvent);
}

/// No-op sink — discards every event.  Used by default when no observer
/// is attached to the loop.
pub struct NullSink;

#[async_trait]
impl EventSink for NullSink {
    async fn emit(&self, _event: AgentEvent) {}
}
