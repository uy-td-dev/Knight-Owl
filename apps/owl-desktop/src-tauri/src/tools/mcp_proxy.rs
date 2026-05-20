//! `McpToolProxy` — wraps one MCP-discovered tool as a [`NativeTool`].
//!
//! Lives in the app layer because it bridges `owl-mcp` and `owl-armory`,
//! which cannot import each other (R-13).

use std::sync::Arc;

use async_trait::async_trait;

use owl_armory::traits::NativeTool;
use owl_armory::ArmoryError;
use owl_mcp::McpClient;
use owl_protocol::tools::{ToolCall, ToolResult};

/// A native-tool façade wrapping one tool discovered from an external MCP server.
pub struct McpToolProxy {
    /// Canonical name from the server's `tools/list` response, leaked to `'static`.
    ///
    /// `Box::leak` is used once at startup — the allocation lives for the
    /// process lifetime, which is acceptable for a small set of tool names.
    name:        &'static str,
    /// Description from `tools/list`, leaked to `'static` for the same reason.
    description: &'static str,
    /// Shared handle to the MCP client for the server that owns this tool.
    client: Arc<McpClient>,
}

impl McpToolProxy {
    /// Construct a proxy, leaking `name` and `description` into static storage.
    ///
    /// Called once at startup during MCP tool discovery — the one-time
    /// allocation cost is acceptable.
    pub fn new(name: String, description: String, client: Arc<McpClient>) -> Self {
        Self {
            name:        Box::leak(name.into_boxed_str()),
            description: Box::leak(description.into_boxed_str()),
            client,
        }
    }
}

#[async_trait]
impl NativeTool for McpToolProxy {
    fn name(&self) -> &'static str { self.name }
    fn description(&self) -> &'static str { self.description }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let output = self
            .client
            .call_tool(&call.name, call.args)
            .await
            .map_err(|e| ArmoryError::Execution(e.to_string()))?;
        Ok(ToolResult::ok(self.name(), output))
    }
}
