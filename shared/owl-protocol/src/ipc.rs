//! Tauri IPC payload types shared between Rust and TypeScript.

use serde::{Deserialize, Serialize};

use crate::attachment::Attachment;

// ─── Chat ─────────────────────────────────────────────────────────────────────

/// Input payload for the `send_message` Tauri command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc", derive(ts_rs::TS))]
#[cfg_attr(feature = "ipc", ts(export))]
pub struct ChatInput {
    /// User message text.
    pub message: String,
    /// Optional conversation session identifier.
    pub session_id: Option<String>,
    /// Optional multi-modal attachments (images, text files).  Vision-capable
    /// models receive them as native content blocks; others get text fallbacks.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

/// Output payload for the `send_message` Tauri command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc", derive(ts_rs::TS))]
#[cfg_attr(feature = "ipc", ts(export))]
pub struct ChatOutput {
    /// Assistant response text.
    pub response: String,
    /// Session identifier (created if not provided in input).
    pub session_id: String,
}

// ─── Tools ────────────────────────────────────────────────────────────────────

/// Origin of a tool (native or from an MCP server).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolSource {
    Native,
    Mcp { server_id: String, server_name: String },
}

/// Metadata about a single tool exposed to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name:        String,
    pub description: String,
    pub source:      ToolSource,
}

// ─── MCP ──────────────────────────────────────────────────────────────────────

/// Transport mechanism for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    #[default]
    Stdio,
    WebSocket,
    Http,
}

/// Configuration for a single MCP server persisted on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique identifier (UUID).
    pub id:        String,
    /// Human-readable display name.
    pub name:      String,
    /// Transport used to reach the server.
    pub transport: McpTransportKind,
    /// Executable path — used for `Stdio` transport.
    pub command:   Option<String>,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args:      Vec<String>,
    /// Environment variables `KEY=VALUE` — used for `Stdio` transport.
    #[serde(default)]
    pub env:       Vec<String>,
    /// WebSocket or HTTP URL — used for `WebSocket`/`Http` transports.
    pub url:       Option<String>,
    /// Whether the server is active.
    #[serde(default = "default_true")]
    pub enabled:   bool,
}

fn default_true() -> bool { true }
