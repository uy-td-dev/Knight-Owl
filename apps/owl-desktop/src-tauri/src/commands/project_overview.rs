//! Project-context loader.
//!
//! Auto-discovers a "what is this project" document from the workspace and
//! prepends it to the agent's system prompt every turn — so the model walks
//! into each conversation already framed, instead of having to `read_file`
//! everything from scratch.
//!
//! Source priority (first existing file wins):
//! 1. `<workspace>/.knight-owl/project_overview.md`  ← user-curated, highest priority
//! 2. `<workspace>/CLAUDE.md`                        ← Anthropic project doc
//! 3. `<workspace>/AGENTS.md`                        ← agent-spec convention
//! 4. `<workspace>/README.md`                        ← generic fallback
//!
//! All sources are truncated to [`MAX_OVERVIEW_CHARS`] before injection so
//! a giant README doesn't dominate the context window.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::state::AppState;

/// Hard cap on how much project text we'll inject.  ~3000 chars ≈ 750
/// tokens — enough for a meaningful summary, small enough that it doesn't
/// crowd out the user's actual prompt + chat history.
pub const MAX_OVERVIEW_CHARS: usize = 3000;

const CURATED_REL: &str = ".knight-owl/project_overview.md";
const FALLBACK_FILES: &[&str] = &["CLAUDE.md", "AGENTS.md", "README.md"];

/// Returns the resolved path of the active project-context source, or
/// `None` if no candidate file exists in the workspace.
pub fn resolve_source(workspace: &Path) -> Option<PathBuf> {
    let curated = workspace.join(CURATED_REL);
    if curated.is_file() { return Some(curated); }
    for name in FALLBACK_FILES {
        let p = workspace.join(name);
        if p.is_file() { return Some(p); }
    }
    None
}

/// Read the active project context from disk.  Returns the trimmed +
/// truncated content, or `None` if no source file exists.
///
/// Cheap on the hot path: small file, OS keeps it in page cache.  Called on
/// every Gemini request so we always inject the latest content — user edits
/// to the source file take effect on the next turn with no restart.
pub fn read_project_context(workspace: &Path) -> Option<String> {
    // Always include an auto-detected stack summary — even when no
    // CLAUDE.md / README is present — so the agent doesn't default to
    // assuming "Rust project" for everything.
    let stack = detect_stack(workspace);

    let path = resolve_source(workspace);
    let doc  = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());

    match (stack, doc) {
        (None, None) => None,
        (stack_opt, doc_opt) => {
            let mut out = String::new();
            if let Some(s) = stack_opt {
                out.push_str(&s);
                out.push_str("\n\n");
            }
            if let Some(raw) = doc_opt {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    let body = if trimmed.len() > MAX_OVERVIEW_CHARS {
                        let mut s = trimmed[..MAX_OVERVIEW_CHARS].to_string();
                        s.push_str("\n\n…(truncated)");
                        s
                    } else { trimmed.to_string() };
                    let label = path.as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("project");
                    out.push_str(&format!("(from `{label}`)\n\n{body}"));
                }
            }
            let s = out.trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
    }
}

/// Cheap workspace stack sniff — checks for marker files and returns a
/// short summary the agent can use to pick the right tools (cargo vs npm
/// vs pytest, etc.) without re-discovering on every turn.
///
/// Returns `None` if no recognised markers are found (empty / docs-only
/// repos), so callers can fall back to README / CLAUDE.md only.
fn detect_stack(workspace: &Path) -> Option<String> {
    let mut langs: Vec<&str> = Vec::new();
    let mut runners: Vec<&str> = Vec::new();

    let has = |rel: &str| workspace.join(rel).exists();

    if has("Cargo.toml")          { langs.push("Rust");       runners.push("cargo"); }
    if has("package.json") {
        langs.push("Node.js/TypeScript");
        runners.push(if has("pnpm-lock.yaml") { "pnpm" }
                     else if has("yarn.lock") { "yarn" }
                     else { "npm" });
    }
    if has("pyproject.toml") || has("setup.py") || has("requirements.txt") {
        langs.push("Python");
        runners.push(if has("pyproject.toml") && has("poetry.lock") { "poetry" }
                     else if has("uv.lock") { "uv" }
                     else { "pip" });
    }
    if has("go.mod")              { langs.push("Go");         runners.push("go"); }
    if has("pom.xml")             { langs.push("Java");       runners.push("mvn"); }
    if has("build.gradle") || has("build.gradle.kts") { langs.push("Java/Kotlin"); runners.push("gradle"); }
    if has("composer.json")       { langs.push("PHP");        runners.push("composer"); }
    if has("Gemfile")             { langs.push("Ruby");       runners.push("bundle"); }
    if has("mix.exs")             { langs.push("Elixir");     runners.push("mix"); }
    if has("Dockerfile") || has("compose.yaml") || has("docker-compose.yml") {
        runners.push("docker");
    }

    if langs.is_empty() && runners.is_empty() { return None; }

    let langs_str   = if langs.is_empty()   { "—".to_string() } else { langs.join(", ") };
    let runners_str = if runners.is_empty() { "—".to_string() } else { runners.join(", ") };
    Some(format!(
        "<project_stack>\n\
         languages: {langs_str}\n\
         runners:   {runners_str}\n\
         workspace: {}\n\
         </project_stack>",
        workspace.display(),
    ))
}

// ─── Tauri commands surfacing the loader to the UI ───────────────────────────

#[derive(serde::Serialize)]
pub struct ProjectContextStatus {
    /// Absolute path of the file currently used (if any).
    pub source: Option<String>,
    /// Display name (e.g. `CLAUDE.md`).
    pub label:  Option<String>,
    /// Full content (post-trim, post-truncate) — empty when no source found.
    pub content: String,
    /// Total size on disk before truncation, in bytes.
    pub raw_size: usize,
    /// Whether truncation happened.
    pub truncated: bool,
}

/// Fetch the active project-context payload for display in the UI.
#[tauri::command]
pub async fn get_project_context(state: State<'_, AppState>) -> Result<ProjectContextStatus, String> {
    let source = resolve_source(&state.workspace);
    match source {
        None => Ok(ProjectContextStatus {
            source: None, label: None,
            content: String::new(), raw_size: 0, truncated: false,
        }),
        Some(path) => {
            let raw  = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let raw_size = raw.len();
            let trimmed = raw.trim();
            let truncated = trimmed.len() > MAX_OVERVIEW_CHARS;
            let body = if truncated {
                format!("{}\n\n…(truncated)", &trimmed[..MAX_OVERVIEW_CHARS])
            } else {
                trimmed.to_string()
            };
            let label = path.file_name().and_then(|n| n.to_str()).map(str::to_string);
            Ok(ProjectContextStatus {
                source: Some(path.display().to_string()),
                label,
                content: body,
                raw_size,
                truncated,
            })
        }
    }
}

/// Persist a user-curated overview at
/// `<workspace>/.knight-owl/project_overview.md` — overrides every fallback.
#[tauri::command]
pub async fn save_project_context(
    content: String,
    state:   State<'_, AppState>,
) -> Result<(), String> {
    let path = state.workspace.join(CURATED_REL);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    tracing::info!(path = %path.display(), "saved curated project overview");
    Ok(())
}
