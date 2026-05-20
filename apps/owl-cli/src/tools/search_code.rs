//! `SearchCodeTool` — query the code graph in SurrealDB by keyword.
//!
//! Lives in owl-cli (not owl-armory) because it depends on owl-vault,
//! which would violate the crate dependency boundary for owl-armory.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use owl_armory::traits::NativeTool;
use owl_armory::ArmoryError;
use owl_protocol::tools::{ToolCall, ToolResult};
use owl_vault::HybridStore;

/// Searches the code graph by keyword, returning matching code nodes.
pub struct SearchCodeTool {
    pub store: Arc<dyn HybridStore>,
}

#[derive(Deserialize)]
struct Args {
    query: String,
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_limit() -> u64 { 10 }

#[async_trait]
impl NativeTool for SearchCodeTool {
    fn name(&self) -> &'static str { "search_code" }

    fn description(&self) -> &'static str {
        "Search the code graph by keyword. Returns matching function, struct, and \
         module names with their file paths and source previews."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = serde_json::from_value(call.args)
            .map_err(|e| ArmoryError::InvalidArgs(e.to_string()))?;

        let nodes = self
            .store
            .search_code_nodes(&args.query, args.limit)
            .await
            .map_err(|e| ArmoryError::Execution(e.to_string()))?;

        let results: Vec<serde_json::Value> = nodes
            .into_iter()
            .map(|n| serde_json::json!({
                "name":       n.name,
                "kind":       format!("{:?}", n.kind),
                "file":       n.file_path,
                "lines":      format!("{}-{}", n.start_line, n.end_line),
                "preview":    n.preview,
            }))
            .collect();

        Ok(ToolResult::ok(self.name(), serde_json::json!({ "results": results })))
    }
}
