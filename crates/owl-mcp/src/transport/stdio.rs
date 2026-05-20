//! Stdio MCP transport — communicates with a child process over stdin/stdout.
//!
//! The MCP wire format is newline-delimited JSON: one [`McpMessage`] object
//! per line.  `send()` writes to stdin and reads one response line from stdout.
//! This is a synchronous request/response pattern; concurrent callers must
//! serialise access via the `Mutex` guards on stdin/stdout.

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use owl_protocol::mcp::{McpError, McpMessage};

use std::process::Stdio;
use std::sync::Arc;

use super::McpTransport;

/// Stdio-based MCP transport.
///
/// Spawns a child process and communicates over its stdin/stdout using
/// newline-delimited JSON (one [`McpMessage`] per line).
pub struct StdioTransport {
    stdin:  Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    // Hold the child so it isn't dropped (and killed) immediately.
    _child: Arc<Mutex<Child>>,
}

impl StdioTransport {
    /// Spawn `cmd args…` and wire up its stdin/stdout as the MCP channel.
    ///
    /// `envs` is a list of `(KEY, VALUE)` pairs injected into the child's
    /// environment.  Callers are responsible for parsing `"KEY=VALUE"` strings
    /// (e.g. from [`McpServerConfig::env`]) before calling this function.
    pub async fn spawn(cmd: &str, args: &[&str], envs: &[(String, String)]) -> Result<Self, McpError> {
        let mut child = Command::new(cmd)
            .args(args)
            .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| McpError::Transport(format!("spawn failed: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("child stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("child stdout unavailable".into()))?;

        Ok(Self {
            stdin:  Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            _child: Arc::new(Mutex::new(child)),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    /// Write `msg` as a JSON line to stdin, then read one JSON line from stdout.
    async fn send(&self, msg: McpMessage) -> Result<McpMessage, McpError> {
        let line = serde_json::to_string(&msg)
            .map_err(|e| McpError::Protocol(format!("serialise: {e}")))?;

        // Write request.
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| McpError::Transport(format!("write: {e}")))?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| McpError::Transport(format!("write newline: {e}")))?;
            stdin
                .flush()
                .await
                .map_err(|e| McpError::Transport(format!("flush: {e}")))?;
        }

        // Read response.
        let mut response_line = String::new();
        {
            let mut stdout = self.stdout.lock().await;
            stdout
                .read_line(&mut response_line)
                .await
                .map_err(|e| McpError::Transport(format!("read: {e}")))?;
        }

        if response_line.is_empty() {
            return Err(McpError::Transport("child process closed stdout".into()));
        }

        serde_json::from_str(response_line.trim())
            .map_err(|e| McpError::Protocol(format!("deserialise: {e}")))
    }

    async fn close(&self) -> Result<(), McpError> {
        // Dropping the stdin signals EOF to the child; the child should exit.
        // We just flush and let Drop handle cleanup.
        let mut stdin = self.stdin.lock().await;
        stdin.flush().await.ok();
        Ok(())
    }
}
