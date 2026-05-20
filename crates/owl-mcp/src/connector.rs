//! Factory: `McpServerConfig` → `McpTransport` → `McpClient`.
//!
//! This is the only place that matches on [`McpTransportKind`] — transport
//! selection is centralised here so `client.rs` never branches on transport
//! type (R-17).

use owl_protocol::ipc::{McpServerConfig, McpTransportKind};
use owl_protocol::mcp::McpError;

use crate::client::McpClient;
use crate::transport::stdio::StdioTransport;

/// Build a live [`McpClient`] from a persisted [`McpServerConfig`].
///
/// Transport selection happens here, never inside `client.rs` (R-17).
/// Returns an error if the config is invalid or the child process cannot
/// be spawned — callers should log and skip rather than propagate fatally.
pub async fn connect(cfg: &McpServerConfig) -> Result<McpClient, McpError> {
    match cfg.transport {
        McpTransportKind::Stdio => {
            let cmd = cfg.command.as_deref().ok_or_else(|| {
                McpError::Transport("stdio transport requires `command` to be set".into())
            })?;
            let args: Vec<&str> = cfg.args.iter().map(String::as_str).collect();
            let envs: Vec<(String, String)> = cfg
                .env
                .iter()
                .filter_map(|kv| {
                    let mut parts = kv.splitn(2, '=');
                    let k = parts.next()?.to_string();
                    let v = parts.next()?.to_string();
                    Some((k, v))
                })
                .collect();
            let transport = StdioTransport::spawn(cmd, &args, &envs).await?;
            Ok(McpClient::new(transport))
        }
        McpTransportKind::WebSocket | McpTransportKind::Http => Err(McpError::Transport(
            format!("transport {:?} is not yet implemented", cfg.transport),
        )),
    }
}
