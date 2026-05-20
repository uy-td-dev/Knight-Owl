//! `GrepTool` — recursive regex content search.
//!
//! Walks the workspace honouring conventional ignore directories (`.git`,
//! `target`, `node_modules`, `dist`, `build`).  Returns matches grouped by
//! file with line numbers, mimicking ripgrep's output but pure-Rust so we
//! don't depend on `rg` being installed on the user's machine.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use regex::RegexBuilder;
use serde::Deserialize;
use walkdir::{DirEntry, WalkDir};

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::tools::read_file::safe_join;
use crate::traits::NativeTool;
use crate::ArmoryError;

const IGNORE_DIRS: &[&str] = &[".git", "target", "node_modules", "dist", "build", ".next", ".venv"];
const MAX_FILE_BYTES: u64   = 2 * 1024 * 1024;
const MAX_TOTAL_MATCHES: usize = 500;

pub struct GrepTool {
    pub workspace_root: PathBuf,
}

#[derive(Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    #[serde(alias = "file_path", alias = "filepath", alias = "filename")]
    path:    Option<String>,
    /// Restrict to files matching this glob (e.g. `"*.rs"`, `"src/**/*.ts"`).
    #[serde(default)]
    glob:    Option<String>,
    /// `"content"` (default) | `"files_with_matches"` | `"count"`.
    #[serde(default)]
    output_mode: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
}

#[async_trait]
impl NativeTool for GrepTool {
    fn name(&self) -> &'static str { "grep" }

    fn description(&self) -> &'static str {
        "Search file contents using a regular expression.  Walks the workspace \
         (skipping .git/target/node_modules/etc.).  `output_mode` may be \
         `content` (default — file:line:text), `files_with_matches`, or `count`."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        let re = RegexBuilder::new(&args.pattern)
            .case_insensitive(args.case_insensitive)
            .multi_line(true)
            .build()
            .map_err(|e| ArmoryError::InvalidArgs(format!("invalid regex: {e}")))?;

        let scan_root = match &args.path {
            Some(rel) => safe_join(&self.workspace_root, rel)?,
            None      => self.workspace_root.clone(),
        };

        let glob_pat = args.glob.as_deref().map(glob::Pattern::new)
            .transpose()
            .map_err(|e| ArmoryError::InvalidArgs(format!("invalid glob: {e}")))?;

        let mode = args.output_mode.as_deref().unwrap_or("content");

        let mut matches:  Vec<serde_json::Value> = Vec::new();
        let mut files_hit: Vec<String> = Vec::new();
        let mut total_count: usize = 0;
        let mut truncated = false;

        for entry in WalkDir::new(&scan_root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_ignored(e))
        {
            let entry = match entry { Ok(e) => e, Err(_) => continue };
            if !entry.file_type().is_file() { continue; }
            let path = entry.path();

            if let Ok(meta) = entry.metadata() {
                if meta.len() > MAX_FILE_BYTES { continue; }
            }

            let rel = path.strip_prefix(&self.workspace_root).unwrap_or(path);
            if let Some(p) = &glob_pat {
                if !p.matches_path(rel) { continue; }
            }

            let content = match tokio::fs::read_to_string(path).await {
                Ok(c)  => c,
                Err(_) => continue, // skip binary / unreadable
            };

            let mut file_count = 0usize;
            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    file_count += 1;
                    total_count += 1;
                    if mode == "content" && matches.len() < MAX_TOTAL_MATCHES {
                        matches.push(serde_json::json!({
                            "file": rel.display().to_string(),
                            "line": i + 1,
                            "text": truncate_line(line, 240),
                        }));
                    }
                    if total_count >= MAX_TOTAL_MATCHES {
                        truncated = true;
                        break;
                    }
                }
            }
            if file_count > 0 {
                files_hit.push(rel.display().to_string());
            }
            if truncated { break; }
        }

        let payload = match mode {
            "files_with_matches" => serde_json::json!({
                "files":     files_hit,
                "truncated": truncated,
            }),
            "count" => serde_json::json!({
                "total":     total_count,
                "files":     files_hit.len(),
                "truncated": truncated,
            }),
            _ => serde_json::json!({
                "matches":   matches,
                "total":     total_count,
                "truncated": truncated,
            }),
        };

        Ok(ToolResult::ok(self.name(), payload))
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

fn truncate_line(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() } else { format!("{}…", &s[..n]) }
}

// keep `Path` import warning-free
#[allow(dead_code)]
fn _force_path_use() -> Option<&'static Path> { None }
