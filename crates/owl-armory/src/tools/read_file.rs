//! `ReadFileTool` — read a file within the allowed workspace root.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::traits::NativeTool;
use crate::ArmoryError;

/// Reads a file and returns its content as a string.
///
/// Paths are resolved relative to `allowed_root`; traversal outside it is
/// rejected with `PathNotAllowed`.
pub struct ReadFileTool {
    /// Absolute workspace root — all paths are resolved relative to this.
    pub allowed_root: PathBuf,
}

#[derive(Deserialize)]
struct Args {
    /// File path relative to the workspace root.  Accepts common LLM
    /// hallucinations (`file_path`, `filepath`, `filename`) so we don't
    /// silently fail when models confidently use a non-canonical key.
    #[serde(alias = "file_path", alias = "filepath", alias = "filename")]
    path: String,
    /// 1-indexed first line to return (default: 1).
    #[serde(default)]
    offset: Option<usize>,
    /// Max lines to return (default: 2000, hard max: 5000).
    #[serde(default)]
    limit:  Option<usize>,
    /// Prepend `<line> | ` to each line (default: true — easier for the agent
    /// to reference exact lines later when calling `edit_file`).
    #[serde(default = "default_true")]
    line_numbers: bool,
}
fn default_true() -> bool { true }

const DEFAULT_LIMIT: usize = 2000;
const MAX_LIMIT:     usize = 5000;

#[async_trait]
impl NativeTool for ReadFileTool {
    fn name(&self) -> &'static str { "read_file" }

    fn description(&self) -> &'static str {
        "Read a file's contents.  By default prefixes each line with its line \
         number (`123 | ...`).  Use `offset` (1-indexed) and `limit` to read a \
         slice of large files."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        let abs = safe_join(&self.allowed_root, &args.path)?;
        let raw = tokio::fs::read_to_string(&abs)
            .await
            .map_err(|e| ArmoryError::Execution(format!("{}: {e}", abs.display())))?;

        let lines: Vec<&str> = raw.lines().collect();
        let total = lines.len();

        let start = args.offset.unwrap_or(1).saturating_sub(1);
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
        let end   = (start + limit).min(total);
        let truncated = end < total;

        let slice = &lines[start..end];
        let content = if args.line_numbers {
            slice.iter().enumerate()
                .map(|(i, l)| format!("{:>5} | {}", start + i + 1, l))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            slice.join("\n")
        };

        Ok(ToolResult::ok(self.name(), serde_json::json!({
            "content":     content,
            "lines":       end - start,
            "total_lines": total,
            "truncated":   truncated,
        })))
    }
}

/// Resolve `rel` under `root`, rejecting any path that escapes the root.
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, ArmoryError> {
    let joined = root.join(rel);
    let canonical = joined.canonicalize().unwrap_or(joined.clone());
    let root_canon = root.canonicalize().unwrap_or(root.to_path_buf());
    if canonical.starts_with(&root_canon) {
        Ok(joined)
    } else {
        Err(ArmoryError::PathNotAllowed(rel.to_string()))
    }
}
