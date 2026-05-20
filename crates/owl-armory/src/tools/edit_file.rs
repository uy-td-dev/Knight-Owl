//! `EditFileTool` — surgical string replacement in a file.
//!
//! The agent supplies an `old_string` (with enough surrounding context to be
//! unique in the file) and a `new_string` to replace it with.  Behaves like
//! Claude Code's `Edit` tool: refuses ambiguous matches unless `replace_all`
//! is set explicitly.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::tools::read_file::safe_join;
use crate::traits::NativeTool;
use crate::ArmoryError;

/// Surgical edit: replace `old_string` with `new_string` in a file.
pub struct EditFileTool {
    pub allowed_root: PathBuf,
}

#[derive(Deserialize)]
struct Args {
    #[serde(alias = "file_path", alias = "filepath", alias = "filename")]
    path:        String,
    old_string:  String,
    new_string:  String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl NativeTool for EditFileTool {
    fn name(&self) -> &'static str { "edit_file" }

    fn description(&self) -> &'static str {
        "Replace a unique substring in a file with new text. \
         Provide enough surrounding context in `old_string` so it appears \
         exactly once in the file; otherwise pass `replace_all: true` to \
         replace every occurrence."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        if args.old_string.is_empty() {
            return Err(ArmoryError::InvalidArgs("old_string must not be empty".into()));
        }
        if args.old_string == args.new_string {
            return Err(ArmoryError::InvalidArgs("old_string and new_string are identical".into()));
        }

        let abs = safe_join(&self.allowed_root, &args.path)?;
        let original = tokio::fs::read_to_string(&abs)
            .await
            .map_err(|e| ArmoryError::Execution(format!("read {}: {e}", abs.display())))?;

        let occurrences = original.matches(&args.old_string).count();
        if occurrences == 0 {
            return Err(ArmoryError::Execution(format!(
                "old_string not found in {}",
                args.path
            )));
        }
        if occurrences > 1 && !args.replace_all {
            return Err(ArmoryError::Execution(format!(
                "old_string matches {occurrences} places; pass replace_all=true or add more context"
            )));
        }

        let updated = if args.replace_all {
            original.replace(&args.old_string, &args.new_string)
        } else {
            original.replacen(&args.old_string, &args.new_string, 1)
        };

        tokio::fs::write(&abs, &updated)
            .await
            .map_err(|e| ArmoryError::Execution(format!("write {}: {e}", abs.display())))?;

        Ok(ToolResult::ok(self.name(), serde_json::json!({
            "path":         args.path,
            "replacements": occurrences.min(if args.replace_all { occurrences } else { 1 }),
            "bytes_after":  updated.len(),
        })))
    }
}
