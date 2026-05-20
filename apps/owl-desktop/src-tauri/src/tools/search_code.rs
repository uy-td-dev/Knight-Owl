//! `SearchCodeTool` — searches the SurrealDB code graph by keyword.
//!
//! Lives in the app layer because it bridges `owl-vault` and `owl-armory`,
//! which cannot import each other (R-13).

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use owl_armory::traits::NativeTool;
use owl_armory::ArmoryError;
use owl_protocol::tools::{ToolCall, ToolResult};

/// Searches the indexed code graph by keyword using BM25 + vector retrieval.
pub struct SearchCodeTool {
    pub store: Arc<dyn owl_vault::HybridStore>,
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_limit() -> u64 { 10 }

#[async_trait]
impl NativeTool for SearchCodeTool {
    fn name(&self) -> &'static str { "search_code" }

    fn description(&self) -> &'static str {
        "Search the code graph by keyword. Returns matching functions, structs, \
         and modules with file paths and source previews."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: SearchArgs = Self::parse_args(call.args)?;
        let nodes = self
            .store
            .search_code_nodes(&args.query, args.limit)
            .await
            .map_err(|e| ArmoryError::Execution(e.to_string()))?;
        let results: Vec<serde_json::Value> = nodes
            .into_iter()
            .map(|n| serde_json::json!({
                "name":    n.name,
                "kind":    format!("{:?}", n.kind),
                "file":    n.file_path,
                "lines":   format!("{}-{}", n.start_line, n.end_line),
                "preview": n.preview,
            }))
            .collect();
        Ok(ToolResult::ok(self.name(), serde_json::json!({ "results": results })))
    }
}
