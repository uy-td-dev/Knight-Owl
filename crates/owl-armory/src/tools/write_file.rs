//! `WriteFileTool` — create or overwrite a file within the allowed workspace root.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::tools::read_file::safe_join;
use crate::traits::NativeTool;
use crate::ArmoryError;

/// Creates or overwrites a file at the given path.
///
/// Parent directories are created if they don't exist.
/// Paths outside `allowed_root` are rejected.
pub struct WriteFileTool {
    /// Absolute workspace root — all paths are resolved relative to this.
    pub allowed_root: PathBuf,
}

#[derive(Deserialize)]
struct Args {
    #[serde(alias = "file_path", alias = "filepath", alias = "filename")]
    path:    String,
    content: String,
}

#[async_trait]
impl NativeTool for WriteFileTool {
    fn name(&self) -> &'static str { "write_file" }

    fn description(&self) -> &'static str {
        "Write content to a file at the given path (relative to workspace root). \
         Creates parent directories as needed."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        let abs = safe_join(&self.allowed_root, &args.path)?;
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ArmoryError::Execution(e.to_string()))?;
        }
        tokio::fs::write(&abs, &args.content)
            .await
            .map_err(|e| ArmoryError::Execution(format!("{}: {e}", abs.display())))?;

        Ok(ToolResult::ok(self.name(), serde_json::json!({
            "written": args.path,
            "bytes":   args.content.len(),
        })))
    }
}
