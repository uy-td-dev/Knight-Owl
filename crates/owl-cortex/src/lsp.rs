//! LSP client for L2 Logic — cross-file REFERENCES, IMPLEMENTS, OVERRIDES.
//!
//! Connects to any Language Server (typically `rust-analyzer`) over stdin/stdout
//! using the LSP wire format:
//!   `Content-Length: N\r\n\r\n<json>`
//!
//! Usage:
//! ```ignore
//! let client = LspClient::start_rust_analyzer(&workspace_root).await?;
//! let locs = client.references(file_uri, line, character).await?;
//! client.shutdown().await.ok();
//! ```
//!
//! The client is optional — wired into [`Ingestor`] via [`Ingestor::with_lsp`].
//! If no LSP client is provided, the ingestor only populates L1 edges and leaves
//! L2 (`REFERENCES`, `IMPLEMENTS`, `OVERRIDES`) empty.

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::debug;

use crate::CortexError;

/// JSON-RPC 2.0 LSP client over a child process' stdin/stdout.
pub struct LspClient {
    stdin:   Arc<Mutex<BufWriter<ChildStdin>>>,
    stdout:  Arc<Mutex<BufReader<ChildStdout>>>,
    _child:  Arc<Mutex<Child>>,
    next_id: AtomicI64,
}

impl LspClient {
    /// Spawn `rust-analyzer` rooted at `workspace` and perform LSP handshake.
    pub async fn start_rust_analyzer(workspace: &Path) -> Result<Self, CortexError> {
        Self::start("rust-analyzer", &[], workspace).await
    }

    /// Spawn an arbitrary LSP server at `cmd` with optional extra `args`.
    pub async fn start(
        cmd: &str,
        args: &[&str],
        workspace: &Path,
    ) -> Result<Self, CortexError> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| CortexError::Lsp(format!("failed to spawn {cmd}: {e}")))?;

        let stdin = child.stdin.take()
            .ok_or_else(|| CortexError::Lsp("no stdin".into()))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| CortexError::Lsp("no stdout".into()))?;

        let client = Self {
            stdin:   Arc::new(Mutex::new(BufWriter::new(stdin))),
            stdout:  Arc::new(Mutex::new(BufReader::new(stdout))),
            _child:  Arc::new(Mutex::new(child)),
            next_id: AtomicI64::new(1),
        };

        // LSP initialize handshake.
        let workspace_uri = path_to_uri(workspace);
        let init_result: Value = client
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": workspace_uri,
                    "capabilities": {
                        "textDocument": {
                            "references": { "dynamicRegistration": false },
                            "implementation": { "dynamicRegistration": false }
                        }
                    }
                }),
            )
            .await?;
        debug!(?init_result, "LSP initialized");

        client.notify("initialized", json!({})).await?;
        Ok(client)
    }

    /// Return all reference locations for the symbol at `(line, character)` in `file`.
    pub async fn references(
        &self,
        file: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<lsp_types::Location>, CortexError> {
        let params = json!({
            "textDocument": { "uri": path_to_uri(file) },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": false }
        });
        let result = self.request("textDocument/references", params).await?;
        let locations: Vec<lsp_types::Location> =
            serde_json::from_value(result).unwrap_or_default();
        Ok(locations)
    }

    /// Return implementation locations for the symbol at `(line, character)`.
    pub async fn implementations(
        &self,
        file: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<lsp_types::Location>, CortexError> {
        let params = json!({
            "textDocument": { "uri": path_to_uri(file) },
            "position": { "line": line, "character": character }
        });
        let result = self.request("textDocument/implementation", params).await?;
        // Server may return Location | Location[] | LocationLink[]
        let locations: Vec<lsp_types::Location> = match result {
            Value::Array(arr) => arr
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect(),
            single => serde_json::from_value(single).ok().into_iter().collect(),
        };
        Ok(locations)
    }

    /// Send an LSP shutdown + exit.
    pub async fn shutdown(&self) -> Result<(), CortexError> {
        self.request("shutdown", Value::Null).await?;
        self.notify("exit", Value::Null).await?;
        Ok(())
    }

    // ── JSON-RPC internals ──────────────────────────────────────────────────

    async fn request(&self, method: &str, params: Value) -> Result<Value, CortexError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.send_message(&msg).await?;

        // Read responses until we get the one with our id.
        loop {
            let response = self.read_message().await?;
            if response.get("id").and_then(Value::as_i64) == Some(id) {
                if let Some(error) = response.get("error") {
                    return Err(CortexError::Lsp(format!("LSP error: {error}")));
                }
                return Ok(response.get("result").cloned().unwrap_or(Value::Null));
            }
            // Ignore notifications and other ids in this simple client.
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), CortexError> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.send_message(&msg).await
    }

    async fn send_message(&self, msg: &Value) -> Result<(), CortexError> {
        let body = serde_json::to_string(msg)
            .map_err(|e| CortexError::Lsp(e.to_string()))?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(header.as_bytes()).await
            .map_err(|e| CortexError::Lsp(e.to_string()))?;
        stdin.write_all(body.as_bytes()).await
            .map_err(|e| CortexError::Lsp(e.to_string()))?;
        stdin.flush().await
            .map_err(|e| CortexError::Lsp(e.to_string()))?;
        Ok(())
    }

    async fn read_message(&self) -> Result<Value, CortexError> {
        let mut stdout = self.stdout.lock().await;
        // Parse headers line by line until we find Content-Length.
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            stdout.read_line(&mut line).await
                .map_err(|e| CortexError::Lsp(e.to_string()))?;
            let line = line.trim_end_matches(|c| c == '\r' || c == '\n');
            if line.is_empty() {
                break; // blank line = end of headers
            }
            if let Some(value) = line.strip_prefix("Content-Length: ") {
                content_length = value.trim().parse().ok();
            }
        }
        let len = content_length
            .ok_or_else(|| CortexError::Lsp("missing Content-Length".into()))?;
        let mut buf = vec![0u8; len];
        stdout.read_exact(&mut buf).await
            .map_err(|e| CortexError::Lsp(e.to_string()))?;
        serde_json::from_slice(&buf)
            .map_err(|e| CortexError::Lsp(format!("json parse: {e}")))
    }
}

/// Convert a filesystem path to an LSP `file://` URI.
fn path_to_uri(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", abs.display())
}
