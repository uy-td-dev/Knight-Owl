//! `ListDirTool` — list a directory's immediate children.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::tools::read_file::safe_join;
use crate::traits::NativeTool;
use crate::ArmoryError;

pub struct ListDirTool {
    pub workspace_root: PathBuf,
}

#[derive(Deserialize)]
struct Args {
    /// Directory to list (relative to workspace).  Accepts common LLM
    /// variants like `dir` / `directory` / `folder` / `dir_path` /
    /// `file_path` so we don't silently fail on non-canonical keys.
    #[serde(default,
        alias = "dir", alias = "directory", alias = "folder",
        alias = "dir_path", alias = "file_path", alias = "filepath")]
    path: Option<String>,
}

#[async_trait]
impl NativeTool for ListDirTool {
    fn name(&self) -> &'static str { "list_dir" }

    fn description(&self) -> &'static str {
        "List the immediate children of a directory in the workspace.  \
         Returns each entry's name, kind (`file` or `dir`), and (for files) size."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        let abs = match args.path {
            Some(p) => safe_join(&self.workspace_root, &p)?,
            None    => self.workspace_root.clone(),
        };

        let mut rd = tokio::fs::read_dir(&abs)
            .await
            .map_err(|e| ArmoryError::Execution(format!("read_dir {}: {e}", abs.display())))?;

        let mut entries: Vec<serde_json::Value> = Vec::new();
        while let Some(entry) = rd.next_entry().await
            .map_err(|e| ArmoryError::Execution(e.to_string()))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip hidden files for cleanliness.
            if name.starts_with('.') { continue; }

            let meta = entry.metadata().await.ok();
            let kind = if meta.as_ref().map(|m| m.is_dir()).unwrap_or(false) { "dir" } else { "file" };
            let size = meta.as_ref().filter(|m| m.is_file()).map(|m| m.len());

            entries.push(serde_json::json!({
                "name": name,
                "kind": kind,
                "size": size,
            }));
        }

        // dirs first, then alphabetical
        entries.sort_by(|a, b| {
            let ak = a["kind"].as_str().unwrap_or("");
            let bk = b["kind"].as_str().unwrap_or("");
            (bk == "dir").cmp(&(ak == "dir"))
                .then_with(|| {
                    a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
                })
        });

        Ok(ToolResult::ok(self.name(), serde_json::json!({
            "entries": entries,
            "count":   entries.len(),
        })))
    }
}
