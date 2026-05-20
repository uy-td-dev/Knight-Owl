//! `GlobTool` — find files in the workspace by glob pattern.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use walkdir::{DirEntry, WalkDir};

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::traits::NativeTool;
use crate::ArmoryError;

const IGNORE_DIRS: &[&str] = &[".git", "target", "node_modules", "dist", "build", ".next", ".venv"];
const MAX_RESULTS: usize = 500;

pub struct GlobTool {
    pub workspace_root: PathBuf,
}

#[derive(Deserialize)]
struct Args {
    /// Glob pattern (e.g. `**/*.rs`, `src/**/*.ts`, `Cargo.toml`).
    #[serde(alias = "glob", alias = "pat", alias = "query")]
    pattern: String,
}

#[async_trait]
impl NativeTool for GlobTool {
    fn name(&self) -> &'static str { "glob" }

    fn description(&self) -> &'static str {
        "Find files in the workspace matching a glob pattern (e.g. `**/*.rs`, \
         `src/**/*.ts`).  Returns paths sorted by modification time (newest first)."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        let pat = glob::Pattern::new(&args.pattern)
            .map_err(|e| ArmoryError::InvalidArgs(format!("invalid glob: {e}")))?;

        let mut hits: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        for entry in WalkDir::new(&self.workspace_root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_ignored(e))
        {
            let entry = match entry { Ok(e) => e, Err(_) => continue };
            if !entry.file_type().is_file() { continue; }
            let rel = entry.path().strip_prefix(&self.workspace_root)
                .unwrap_or(entry.path());
            if pat.matches_path(rel) {
                let mtime = entry.metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                hits.push((rel.to_path_buf(), mtime));
            }
            if hits.len() >= MAX_RESULTS { break; }
        }

        // newest first
        hits.sort_by(|a, b| b.1.cmp(&a.1));
        let truncated = hits.len() >= MAX_RESULTS;
        let files: Vec<String> = hits.into_iter().map(|(p, _)| p.display().to_string()).collect();

        Ok(ToolResult::ok(self.name(), serde_json::json!({
            "files":     files,
            "truncated": truncated,
        })))
    }
}

fn is_ignored(entry: &DirEntry) -> bool {
    if entry.file_type().is_dir() {
        if let Some(name) = entry.path().file_name().and_then(|s| s.to_str()) {
            if IGNORE_DIRS.contains(&name) { return true; }
            if name.starts_with('.') && name.len() > 1 { return true; }
        }
    }
    false
}
