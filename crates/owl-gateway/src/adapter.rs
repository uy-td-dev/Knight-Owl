//! `MessagingAdapter` — the contract every platform impl satisfies.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors returned by adapters / bridge.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum GatewayError {
    #[error("gateway transport error: {0}")]
    Transport(String),
    #[error("gateway config error: {0}")]
    Config(String),
}

/// One message handed up to the bridge from a platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    /// Stable platform-scoped chat id (Telegram chat id, Discord channel id, etc.).
    /// Bridge uses this as the reply target.
    pub chat_id: String,
    /// User-supplied display name, when the platform reports one.
    pub from: Option<String>,
    /// Raw text payload.  Voice / image payloads are converted upstream
    /// or rejected by the adapter for now.
    pub text: String,
}

/// One reply pushed back out to the platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingReply {
    pub chat_id: String,
    pub text:    String,
}

/// Adapter contract.  Each platform owns receiving + sending; the bridge
/// orchestrates the conversion to/from the agent.
#[async_trait]
pub trait MessagingAdapter: Send + Sync {
    /// Block until at least one message is available, then return ALL the
    /// buffered messages in one batch.  An empty batch indicates the
    /// adapter has been shut down and the bridge should exit.
    async fn poll(&self) -> Result<Vec<IncomingMessage>, GatewayError>;

    /// Push a reply back.  Adapters that don't support replies (e.g.
    /// one-way log adapters) may silently drop or log.
    async fn send(&self, reply: OutgoingReply) -> Result<(), GatewayError>;

    /// Short, lowercase platform tag for logging ("telegram", "log", …).
    fn label(&self) -> &'static str;
}
