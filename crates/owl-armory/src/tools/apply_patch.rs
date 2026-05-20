//! `ApplyPatchTool` — apply a unified-diff patch to one or more files.
//!
//! Accepts the standard `--- a/path` / `+++ b/path` header followed by `@@`
//! hunks.  This is the most token-efficient way to express many small,
//! scattered edits across a file.
//!
//! Validation rules:
//! - Every hunk's context (` `) and removal (`-`) lines must match the file
//!   exactly at the hunk's reported line.  If the file has drifted we abort
//!   without writing anything.
//! - All hunks for all files are validated and staged in memory first; the
//!   patch is committed only after every hunk applies cleanly.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::tools::read_file::safe_join;
use crate::traits::NativeTool;
use crate::ArmoryError;

pub struct ApplyPatchTool {
    pub allowed_root: PathBuf,
}

#[derive(Deserialize)]
struct Args {
    /// A unified diff (output of `diff -u` / `git diff`).  Headers and hunks
    /// for one or more files are accepted.
    patch: String,
}

#[async_trait]
impl NativeTool for ApplyPatchTool {
    fn name(&self) -> &'static str { "apply_patch" }

    fn description(&self) -> &'static str {
        "Apply a unified-diff patch to one or more files.  Accepts standard \
         `--- a/path` / `+++ b/path` headers followed by `@@` hunks. \
         Most token-efficient option for many scattered changes; prefer \
         `edit_file` or `multi_edit` for one or two single-region edits."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        let patches = parse_patch(&args.patch)?;
        if patches.is_empty() {
            return Err(ArmoryError::InvalidArgs(
                "patch contains no `--- /+++` file headers".into()
            ));
        }

        // Stage every file in memory; only write once everything validates.
        let mut staged: HashMap<String, String> = HashMap::new();
        let mut summary: Vec<serde_json::Value> = Vec::new();

        for fp in &patches {
            let abs = safe_join(&self.allowed_root, &fp.path)?;
            let original = tokio::fs::read_to_string(&abs)
                .await
                .map_err(|e| ArmoryError::Execution(format!("read {}: {e}", abs.display())))?;

            let new_content = apply_hunks(&original, &fp.hunks)
                .map_err(|e| ArmoryError::Execution(format!("{}: {e}", fp.path)))?;

            let added: usize = fp.hunks.iter().map(|h| h.added).sum();
            let removed: usize = fp.hunks.iter().map(|h| h.removed).sum();
            summary.push(serde_json::json!({
                "path": fp.path, "hunks": fp.hunks.len(),
                "added": added, "removed": removed,
            }));
            staged.insert(fp.path.clone(), new_content);
        }

        // All hunks validated — commit.
        for (rel, content) in &staged {
            let abs = safe_join(&self.allowed_root, rel)?;
            tokio::fs::write(&abs, content).await
                .map_err(|e| ArmoryError::Execution(format!("write {rel}: {e}")))?;
        }

        Ok(ToolResult::ok(self.name(), serde_json::json!({
            "files":  summary,
            "count":  staged.len(),
        })))
    }
}

// ── Patch parser ─────────────────────────────────────────────────────────────

#[derive(Debug)]
struct FilePatch {
    path:  String,
    hunks: Vec<Hunk>,
}

#[derive(Debug)]
struct Hunk {
    /// 1-indexed start line in the *original* file.
    old_start: usize,
    /// Lines in this hunk: each is one of `Context(s)`, `Remove(s)`, `Add(s)`.
    lines: Vec<HunkLine>,
    added:   usize,
    removed: usize,
}

#[derive(Debug)]
enum HunkLine { Context(String), Remove(String), Add(String) }

fn parse_patch(patch: &str) -> Result<Vec<FilePatch>, ArmoryError> {
    let mut out: Vec<FilePatch> = Vec::new();
    let mut current: Option<FilePatch> = None;
    let mut current_hunk: Option<Hunk> = None;

    let mut lines = patch.lines().peekable();
    while let Some(line) = lines.next() {
        // File header — `--- a/path` followed by `+++ b/path`.
        if let Some(rest) = line.strip_prefix("--- ") {
            // Look ahead for the `+++` line.
            let plus = lines.next().ok_or_else(|| ArmoryError::InvalidArgs(
                "patch ended after `---` header".into()
            ))?;
            let dst = plus.strip_prefix("+++ ").ok_or_else(|| ArmoryError::InvalidArgs(
                format!("expected `+++ …` after `--- …`, got: {plus}")
            ))?;
            // Pick the destination path (strip optional `a/` or `b/` prefix).
            let path = strip_diff_prefix(dst).or_else(|| strip_diff_prefix(rest))
                .ok_or_else(|| ArmoryError::InvalidArgs(
                    "could not determine file path from headers".into()
                ))?;

            if let Some(mut prev) = current.take() {
                if let Some(h) = current_hunk.take() { prev.hunks.push(h); }
                out.push(prev);
            }
            current = Some(FilePatch { path, hunks: Vec::new() });
            continue;
        }

        // Hunk header — `@@ -OLD,N +NEW,M @@`.
        if line.starts_with("@@") {
            let old_start = parse_hunk_header(line)?;
            if let Some(h) = current_hunk.take() {
                current.as_mut().unwrap().hunks.push(h);
            }
            current_hunk = Some(Hunk {
                old_start, lines: Vec::new(), added: 0, removed: 0,
            });
            continue;
        }

        // Body of a hunk.
        if let Some(h) = current_hunk.as_mut() {
            if let Some(rest) = line.strip_prefix('+') {
                h.lines.push(HunkLine::Add(rest.to_string()));
                h.added += 1;
            } else if let Some(rest) = line.strip_prefix('-') {
                h.lines.push(HunkLine::Remove(rest.to_string()));
                h.removed += 1;
            } else if let Some(rest) = line.strip_prefix(' ') {
                h.lines.push(HunkLine::Context(rest.to_string()));
            } else if line.is_empty() {
                h.lines.push(HunkLine::Context(String::new()));
            } else if line.starts_with("\\ No newline") {
                // ignore EOF marker
            } else {
                // Unknown line outside a recognised prefix — skip.
            }
        }
    }

    if let Some(mut prev) = current {
        if let Some(h) = current_hunk { prev.hunks.push(h); }
        out.push(prev);
    }
    Ok(out)
}

fn strip_diff_prefix(s: &str) -> Option<String> {
    // `--- a/path/to/file\t<timestamp>` or `--- /dev/null`.
    let path = s.split('\t').next().unwrap_or(s).trim();
    if path == "/dev/null" { return None; }
    let stripped = path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path);
    Some(stripped.to_string())
}

fn parse_hunk_header(line: &str) -> Result<usize, ArmoryError> {
    // `@@ -<start>,<count> +<start>,<count> @@ <ctx>`
    let after = line.trim_start_matches('@').trim_start();
    let minus = after.split_whitespace().next().ok_or_else(|| ArmoryError::InvalidArgs(
        format!("malformed hunk header: {line}")
    ))?;
    let nums = minus.trim_start_matches('-');
    let start = nums.split(',').next().unwrap_or("0").parse::<usize>()
        .map_err(|_| ArmoryError::InvalidArgs(format!("bad hunk start: {line}")))?;
    Ok(start.max(1))
}

// ── Hunk application ────────────────────────────────────────────────────────

fn apply_hunks(original: &str, hunks: &[Hunk]) -> Result<String, String> {
    let orig_lines: Vec<&str> = original.lines().collect();
    let trailing_newline = original.ends_with('\n');

    // Cursor in the original file (0-indexed).
    let mut cursor = 0usize;
    let mut out:    Vec<String> = Vec::new();

    for hunk in hunks {
        let target = hunk.old_start.saturating_sub(1);
        if target < cursor {
            return Err(format!("hunk at line {} overlaps with previous hunk", hunk.old_start));
        }
        // Copy unchanged lines up to the hunk start.
        while cursor < target && cursor < orig_lines.len() {
            out.push(orig_lines[cursor].to_string());
            cursor += 1;
        }

        // Validate context+remove lines match, then emit add+context.
        for hline in &hunk.lines {
            match hline {
                HunkLine::Context(s) => {
                    let actual = orig_lines.get(cursor).copied().unwrap_or("");
                    if actual != s {
                        return Err(format!(
                            "context mismatch at line {}: expected `{}`, found `{}`",
                            cursor + 1, snippet(s), snippet(actual)
                        ));
                    }
                    out.push(s.clone());
                    cursor += 1;
                }
                HunkLine::Remove(s) => {
                    let actual = orig_lines.get(cursor).copied().unwrap_or("");
                    if actual != s {
                        return Err(format!(
                            "removed-line mismatch at line {}: expected `{}`, found `{}`",
                            cursor + 1, snippet(s), snippet(actual)
                        ));
                    }
                    cursor += 1;
                }
                HunkLine::Add(s) => out.push(s.clone()),
            }
        }
    }

    // Append the rest of the file untouched.
    while cursor < orig_lines.len() {
        out.push(orig_lines[cursor].to_string());
        cursor += 1;
    }

    let mut joined = out.join("\n");
    if trailing_newline { joined.push('\n'); }
    Ok(joined)
}

fn snippet(s: &str) -> String {
    if s.len() <= 60 { s.to_string() } else { format!("{}…", &s[..60]) }
}
