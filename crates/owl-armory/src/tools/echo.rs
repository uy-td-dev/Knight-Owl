//! Echo tool — returns its input as output. Useful for smoke-testing the pipeline.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::traits::NativeTool;
use crate::ArmoryError;

/// Input for the echo tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EchoArgs {
    /// The text to echo back.
    pub message: String,
}

/// Echoes `args.message` back as the tool output.
pub struct EchoTool;

#[async_trait]
impl NativeTool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Echoes the provided message back as the tool result."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: EchoArgs = Self::parse_args(call.args)?;
        Ok(ToolResult::ok("echo", serde_json::json!({ "echo": args.message })))
    }
}
