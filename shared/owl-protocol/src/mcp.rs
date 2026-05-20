//! MCP (Model Context Protocol) message types.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A message exchanged with an external MCP server (JSON-RPC 2.0 style).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpMessage {
    /// Optional request id for correlating responses to requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id:      Option<u64>,
    /// JSON-RPC method or event name.
    pub method:  String,
    /// Message payload.
    pub payload: serde_json::Value,
}

/// A tool advertised by an MCP server via `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    /// Canonical tool name as reported by the MCP server.
    pub name:         String,
    /// Human-readable description.
    pub description:  String,
    /// JSON Schema describing the tool's input arguments.
    pub input_schema: serde_json::Value,
}

/// Errors that can occur during MCP communication.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum McpError {
    #[error("transport error: {0}")]
    Transport(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("server error: {0}")]
    Server(String),

    #[error("timeout")]
    Timeout,
}
