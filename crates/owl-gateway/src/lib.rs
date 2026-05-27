//! Owl-Gateway — bridges external messaging platforms to the agent loop.
//!
//! Architecture:
//! ```text
//! Telegram /  Discord  ──┐
//! Slack    /  Email    ──┼──> MessagingAdapter ──> Bridge ──> AgentRunner
//! CLI shim             ──┘
//! ```
//!
//! Adding a platform = one [`MessagingAdapter`] impl + a thin bootstrap.
//! Adapters are intentionally simple: pull incoming messages, push outgoing
//! replies — no policy logic, no rate limiting; that lives in `Bridge`.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod bridge;
pub mod log_adapter;

#[cfg(feature = "telegram")]
pub mod telegram;

pub use adapter::{IncomingMessage, MessagingAdapter, OutgoingReply, GatewayError};
pub use bridge::Bridge;
pub use log_adapter::LogAdapter;
