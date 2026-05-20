//! MCP client — wraps external MCP server tools as owl-brain-compatible tool calls.
//!
//! Only the `McpTransport` trait is used here — never a concrete transport type.

use owl_protocol::mcp::{McpError, McpMessage, McpToolDef};

use crate::error::McpClientError;
use crate::transport::McpTransport;

/// Client that communicates with an external MCP server.
pub struct McpClient {
    transport: Box<dyn McpTransport + Send + Sync>,
}

impl McpClient {
    /// Construct a new client with the given transport.
    pub fn new(transport: impl McpTransport + Send + Sync + 'static) -> Self {
        Self { transport: Box::new(transport) }
    }

    /// Send a raw MCP message, with basic retry on transport errors.
    pub async fn send(&self, msg: McpMessage) -> Result<McpMessage, McpClientError> {
        // Retry once on transient transport errors.
        match self.transport.send(msg.clone()).await {
            Ok(resp) => Ok(resp),
            Err(McpError::Transport(_)) => {
                self.transport.send(msg).await.map_err(McpClientError::Transport)
            }
            Err(e) => Err(McpClientError::Transport(e)),
        }
    }

    /// Discover all tools advertised by the connected MCP server.
    ///
    /// Sends `tools/list` and deserializes the response payload as a
    /// `Vec<McpToolDef>`.  Uses `id: 1` for request correlation.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, McpClientError> {
        let req = McpMessage {
            id:      Some(1),
            method:  "tools/list".into(),
            payload: serde_json::Value::Null,
        };
        let resp = self.send(req).await?;
        serde_json::from_value(resp.payload).map_err(McpClientError::Serialization)
    }

    /// Invoke a named tool on the MCP server with JSON arguments.
    ///
    /// Sends `tools/call` with `{ "name": name, "arguments": args }`.
    /// Uses `id: 2` for request correlation.
    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, McpClientError> {
        let req = McpMessage {
            id:      Some(2),
            method:  "tools/call".into(),
            payload: serde_json::json!({ "name": name, "arguments": args }),
        };
        let resp = self.send(req).await?;
        Ok(resp.payload)
    }

    /// Close the underlying transport gracefully.
    pub async fn close(&self) -> Result<(), McpClientError> {
        self.transport.close().await.map_err(McpClientError::Transport)
    }
}
