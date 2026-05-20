//! `MultiEditTool` — apply several string replacements to one file atomically.
//!
//! Each edit is applied sequentially against the in-memory result of the
//! previous edit, so later edits can target text introduced by earlier ones.
//! If *any* edit fails (old_string not found, ambiguous match without
//! `replace_all`, etc.) the file is left untouched — nothing is written until
//! every edit has succeeded.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::tools::read_file::safe_join;
use crate::traits::NativeTool;
use crate::ArmoryError;

pub struct MultiEditTool {
    pub allowed_root: PathBuf,
}

#[derive(Deserialize)]
struct Args {
    #[serde(alias = "file_path", alias = "filepath", alias = "filename")]
    path:  String,
    edits: Vec<EditOp>,
}

#[derive(Deserialize)]
struct EditOp {
    old_string:  String,
    new_string:  String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl NativeTool for MultiEditTool {
    fn name(&self) -> &'static str { "multi_edit" }

    fn description(&self) -> &'static str {
        "Apply multiple string replacements to a single file atomically.  \
         Edits run in order, each against the result of the previous, and the \
         file is only written if every edit succeeds.  Cheaper than calling \
         `edit_file` repeatedly: one tool call, one file read, one file write."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        if args.edits.is_empty() {
            return Err(ArmoryError::InvalidArgs("edits must not be empty".into()));
        }

        let abs = safe_join(&self.allowed_root, &args.path)?;
        let mut content = tokio::fs::read_to_string(&abs)
            .await
            .map_err(|e| ArmoryError::Execution(format!("read {}: {e}", abs.display())))?;

        let mut total_replacements = 0usize;
        for (i, op) in args.edits.iter().enumerate() {
            if op.old_string.is_empty() {
                return Err(ArmoryError::InvalidArgs(
                    format!("edit #{}: old_string must not be empty", i + 1)
                ));
            }
            if op.old_string == op.new_string { continue; }

            let occurrences = content.matches(&op.old_string).count();
            if occurrences == 0 {
                return Err(ArmoryError::Execution(format!(
                    "edit #{}: old_string not found", i + 1
                )));
            }
            if occurrences > 1 && !op.replace_all {
                return Err(ArmoryError::Execution(format!(
                    "edit #{}: matches {occurrences} places — set replace_all or add context",
                    i + 1
                )));
            }

            content = if op.replace_all {
                total_replacements += occurrences;
                content.replace(&op.old_string, &op.new_string)
            } else {
                total_replacements += 1;
                content.replacen(&op.old_string, &op.new_string, 1)
            };
        }

        tokio::fs::write(&abs, &content)
            .await
            .map_err(|e| ArmoryError::Execution(format!("write {}: {e}", abs.display())))?;

        Ok(ToolResult::ok(self.name(), serde_json::json!({
            "path":         args.path,
            "edits":        args.edits.len(),
            "replacements": total_replacements,
            "bytes_after":  content.len(),
        })))
    }
}
