//! Trait contract for all native tools.

use async_trait::async_trait;
use serde::de::DeserializeOwned;

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::ArmoryError;

/// Metadata and dispatch contract for a native tool.
#[async_trait]
pub trait NativeTool: Send + Sync {
    /// Unique tool name (matches `rig::tool::Tool::NAME`).
    fn name(&self) -> &'static str;

    /// Human-readable description used in tool listings.
    fn description(&self) -> &'static str;

    /// Execute the tool with a protocol-level `ToolCall`.
    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError>;

    /// Deserialize `args` into `T`, mapping serde errors to
    /// [`ArmoryError::InvalidArgs`].
    ///
    /// Call `Self::parse_args(call.args)?` inside `run()` to eliminate
    /// per-tool boilerplate deserialization.  Uses `where Self: Sized` so
    /// the generic doesn't break dyn-compatibility — `Box<dyn NativeTool>`
    /// stays valid because this method is never called through a trait
    /// object (only via the concrete tool type from inside its own `run`).
    fn parse_args<T: DeserializeOwned>(args: serde_json::Value) -> Result<T, ArmoryError>
    where
        Self: Sized,
    {
        serde_json::from_value(args).map_err(|e| ArmoryError::InvalidArgs(e.to_string()))
    }
}
