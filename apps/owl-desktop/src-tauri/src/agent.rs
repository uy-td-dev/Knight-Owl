//! Agentic brain — proper multi-turn chat with native Gemini function calling.
//!
//! Loop:
//!   1. Append user message to history.
//!   2. Send `contents` (full history) + `tools` (function declarations) to Gemini.
//!   3. If Gemini returns one or more `functionCall` parts → execute each tool,
//!      append the call + result to history as a `model` / `function` turn,
//!      jump back to step 2.
//!   4. When Gemini returns a `text` part → stream it word-by-word to the UI,
//!      append it to history, persist, return.
//!
//! History is persisted to `~/.knight-owl/chat.json` after every turn so the
//! agent retains memory across app restarts.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use owl_armory::traits::NativeTool;
use owl_protocol::tools::ToolCall as ProtoToolCall;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::time::sleep;

use crate::state::ChatTurn;

// ── Public stream chunk emitted to the UI ────────────────────────────────────

#[derive(Clone, Default, Serialize)]
pub struct StreamChunk {
    pub id:        String,
    pub text:      String,
    pub thinking:  Option<String>,
    /// Tool name when a function call fires.  Paired with `tool_args` /
    /// `tool_result` so the UI can show what the agent did.
    pub tool_call:   Option<String>,
    /// JSON-stringified arguments the agent passed to the tool.
    pub tool_args:   Option<String>,
    /// JSON-stringified (truncated) result returned by the tool.
    pub tool_result: Option<String>,
    pub done:        bool,
    pub error:       Option<String>,
    /// Aggregated input tokens for this turn (Gemini `promptTokenCount`).
    /// Populated only on the terminal `done = true` chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens:  Option<u64>,
    /// Aggregated output tokens (`candidatesTokenCount + thoughtsTokenCount`).
    /// Populated only on the terminal `done = true` chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

// ── Configuration constants ──────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "\
You are Knight-Owl, an agentic coding assistant in the spirit of Claude Code.

# Toolbox
- `list_dir`     — list a directory's contents
- `glob`         — find files by pattern (e.g. `**/*.rs`)
- `grep`         — regex search over file contents (skips .git/target/node_modules)
- `read_file`    — read a file with line numbers; supports `offset`+`limit`
- `edit_file`    — surgical string-replace (one change, unique `old_string`)
- `multi_edit`   — several string-replaces in ONE file in a single call (use when ≥ 2 edits to same file)
- `apply_patch`  — unified-diff patch across one or many files (cheapest for scattered changes)
- `write_file`   — create or fully overwrite a file
- `bash`         — run any shell command (60s default timeout)
- `run_command`  — structured cargo / git invocations
- `search_code`  — semantic graph search (when the project graph is populated)

# Workflow for code tasks
1. **Locate** — use `glob` / `grep` / `list_dir` to discover relevant files.
2. **Understand** — `read_file` the candidates *before* changing them. Never edit
   what you haven't read; never invent content.
3. **Plan** — say in one or two sentences what you will do.
4. **Apply** — pick the cheapest edit primitive that fits:
     • 1 change to 1 file → `edit_file`
     • 2+ changes to the SAME file → `multi_edit` (one read, one write)
     • Scattered changes across one or many files → `apply_patch` (unified diff,
       most compact representation; saves the most tokens)
     • New file or full rewrite → `write_file`
   Never edit what you haven't read; never invent file content.
5. **Verify** — run `cargo check`, tests, lints, or the project's own scripts via
   `run_command` / `bash`.  If anything fails, diagnose and iterate.
6. **Report** — summarise the change, cite `path:line` for every modification, and
   show key diffs in fenced code blocks.

# Style rules
- Reply in plain conversational tone for greetings or general questions.
- For code answers, use Markdown: headings, lists, fenced ``` blocks with
  language, tables when comparing.
- Never paste a file's full contents back at the user — show only the diff or the
  changed region.
- Remember everything from earlier in the conversation; refer to prior context
  by name when relevant.
- Be honest about uncertainty.  If a tool fails, say so and adjust.

You are NOT a passive chatbot. Plan, act, verify, then explain.

# Authoring orchestra files (.knight-owl/)

Workspace may contain agents/skills/workflows/commands under `.knight-owl/`.
When asked to create or edit any of these:
- **Agent** = folder `agents/<id>/` with TWO files: `agent.toml` + `system.md`.
  Always write BOTH (use `multi_edit` or two `write_file` calls).
- **Skill / Command** = single `.md` with `+++ TOML +++` front-matter.
- **Workflow** = single `.toml` file.
- IDs match `^[a-z][a-z0-9_-]+$`. Folder name MUST equal declared id.
- Always declare `schema_version = 1`.
- Before writing a new file, `read_file` an existing one of the same kind to
  match the exact format. The loader rejects anything off-spec.";

const MAX_HISTORY_TURNS: usize = 60;
const MAX_TOOL_LOOPS:    usize = 8;

/// Per-turn overrides supplied by the orchestra registry.
///
/// When the user picks an agent for a conversation, the frontend sends its
/// id with each `stream_message`; the backend resolves the registered
/// [`owl_protocol::orchestra::AgentSpec`], composes the system prompt with
/// its skills, and packs both into this struct.  `agent.rs` then swaps the
/// hardcoded `SYSTEM_PROMPT` + full tool list for these values.
///
/// `None` everywhere is the back-compat path — preserves the implicit
/// "default" agent behaviour exactly.
#[derive(Debug, Clone)]
pub struct AgentOverrides {
    /// System prompt composed from `system.md` body + every active skill body.
    pub system_prompt: String,
    /// Allowed tool names.  Empty = all allowed (matches FilteredExecutor
    /// semantics and the pre-orchestra default behaviour).
    pub allowed_tools: HashSet<String>,
}

// ── Tool schema declarations (JSON Schema for Gemini function calling) ───────

/// Build the `tools` field for Gemini from the in-app tool registry.
///
/// We hand-write the schemas because [`NativeTool`] doesn't yet expose them.
/// The names here MUST match `NativeTool::name()` exactly.
fn function_declarations() -> Vec<Value> {
    vec![
        json!({
            "name": "read_file",
            "description": "Read a file's contents with line numbers.  Use `offset` (1-indexed first line) and `limit` (max 5000) to slice large files.  Always read before editing.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "path":   { "type": "STRING", "description": "Workspace-relative path." },
                    "offset": { "type": "INTEGER", "description": "First line to read (1-indexed)." },
                    "limit":  { "type": "INTEGER", "description": "Max lines to return." }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "write_file",
            "description": "Create or overwrite a file.  Prefer `edit_file` for surgical changes to existing files.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "path":    { "type": "STRING" },
                    "content": { "type": "STRING" }
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "name": "edit_file",
            "description": "Replace `old_string` with `new_string` in a file.  `old_string` must include enough surrounding context to match exactly once unless `replace_all` is true.  Use for ONE small change.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "path":        { "type": "STRING" },
                    "old_string":  { "type": "STRING", "description": "Exact substring to replace; include 2-3 lines of context for uniqueness." },
                    "new_string":  { "type": "STRING" },
                    "replace_all": { "type": "BOOLEAN", "description": "Replace every occurrence instead of failing on ambiguity." }
                },
                "required": ["path", "old_string", "new_string"]
            }
        }),
        json!({
            "name": "multi_edit",
            "description": "Apply several string replacements to one file in a single tool call.  Edits are applied sequentially and atomically (file is only written if every edit succeeds).  CHEAPER than calling `edit_file` repeatedly — prefer this when changing the same file in 2+ places.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "path":  { "type": "STRING" },
                    "edits": {
                        "type": "ARRAY",
                        "items": {
                            "type": "OBJECT",
                            "properties": {
                                "old_string":  { "type": "STRING" },
                                "new_string":  { "type": "STRING" },
                                "replace_all": { "type": "BOOLEAN" }
                            },
                            "required": ["old_string", "new_string"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }
        }),
        json!({
            "name": "apply_patch",
            "description": "Apply a unified-diff patch (output of `diff -u` / `git diff`) to one OR MORE files.  MOST TOKEN-EFFICIENT for many scattered changes across a file (or several files).  Headers are `--- a/path` / `+++ b/path` followed by `@@ -OLD,N +NEW,M @@` hunks.  Context lines (` `), removals (`-`), and additions (`+`) must align with the file exactly.  Aborts cleanly if any hunk fails to validate.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "patch": { "type": "STRING", "description": "The unified diff text." }
                },
                "required": ["patch"]
            }
        }),
        json!({
            "name": "list_dir",
            "description": "List the immediate children of a directory.  Returns name, kind (file/dir), and size.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "path": { "type": "STRING", "description": "Workspace-relative path; omit for the workspace root." }
                }
            }
        }),
        json!({
            "name": "glob",
            "description": "Find files in the workspace matching a glob pattern (e.g. `**/*.rs`, `src/**/*.ts`).  Sorted newest first.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "pattern": { "type": "STRING" }
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "grep",
            "description": "Recursive regex search over file contents.  Skips .git/target/node_modules.  `output_mode`: `content` (file:line:text), `files_with_matches`, or `count`.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "pattern":          { "type": "STRING", "description": "Rust regex." },
                    "path":             { "type": "STRING", "description": "Restrict scan to this subdirectory." },
                    "glob":             { "type": "STRING", "description": "Restrict to files matching this glob (e.g. `*.rs`)." },
                    "output_mode":      { "type": "STRING", "description": "content | files_with_matches | count" },
                    "case_insensitive": { "type": "BOOLEAN" }
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "bash",
            "description": "Run an arbitrary shell command (`/bin/sh -c <command>`) in the workspace.  Default 60s timeout.  Use this for compilers, formatters, package managers, custom scripts — anything outside of cargo/git.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "command":    { "type": "STRING" },
                    "timeout_ms": { "type": "INTEGER", "description": "Override timeout in milliseconds (max 600000)." }
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "run_command",
            "description": "Run a whitelisted program (cargo or git) with structured args.  Safer than `bash` for these specific tools.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "program": { "type": "STRING", "description": "cargo | git" },
                    "args":    { "type": "ARRAY",  "items": { "type": "STRING" } }
                },
                "required": ["program", "args"]
            }
        }),
        json!({
            "name": "search_code",
            "description": "Search the project's code graph (built by owl-cortex) by keyword.  Faster than grep for symbols when the graph is populated.  Falls back: if you don't get results, use `grep` instead.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "query": { "type": "STRING" },
                    "limit": { "type": "INTEGER" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "spawn_agent",
            "description": "Delegate a self-contained sub-task to a specialised agent registered under .knight-owl/agents/.  Pick `profile` from the workspace's agent ids (e.g. `researcher`, `reviewer`).  The sub-agent runs to completion (max 120s, isolated history, its own tool allowlist) and returns its final answer here so you can quote / summarise it.  Use this for parallel research, narrow audits, or anything that doesn't need full chat context.  Do NOT use it as a thin wrapper around `read_file` / `bash` — only delegate when a specialist persona genuinely helps.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "description": { "type": "STRING", "description": "One-line summary of the sub-task — shown in the parent's tool chip." },
                    "prompt":      { "type": "STRING", "description": "Full instruction to hand to the sub-agent.  Include all the context it needs; it does NOT see the parent's chat history." },
                    "profile":     { "type": "STRING", "description": "Registered agent id (folder name under .knight-owl/agents/)." }
                },
                "required": ["description", "prompt", "profile"]
            }
        }),
    ]
}

// ── Public entry point ───────────────────────────────────────────────────────

pub async fn run_agent(
    req_id:   String,
    user_msg: String,
    app:      AppHandle,
    api_key:  String,
    model:    String,
    tools:    Arc<std::collections::HashMap<String, Box<dyn NativeTool>>>,
    history:  Arc<tokio::sync::Mutex<Vec<ChatTurn>>>,
    history_path: std::path::PathBuf,
    overrides: Option<AgentOverrides>,
    // Truncated workspace overview (CLAUDE.md / README.md / curated).
    // `None` skips injection; `Some` content is prepended to every system
    // prompt so the agent walks in already framed about the project.
    project_context: Option<String>,
) {
    // 1. Append user turn.
    {
        let mut h = history.lock().await;
        h.push(ChatTurn { role: "user".into(), text: user_msg.clone() });
        let len = h.len();
        if len > MAX_HISTORY_TURNS {
            h.drain(0..len - MAX_HISTORY_TURNS);
        }
        sanitize_history(&mut h);
    }

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
    {
        Ok(c)  => c,
        Err(e) => return done(&app, &req_id, Some(e.to_string())),
    };

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/\
         {model}:generateContent?key={api_key}"
    );

    // Aggregated token usage across every loop iteration of this turn.
    // Sent up to the UI on the terminal `done = true` chunk so the chat
    // sidebar can show real cost instead of the 4-chars-per-token estimate.
    let mut total_input_tokens:  u64 = 0;
    let mut total_output_tokens: u64 = 0;

    // 2. Tool loop.
    for loop_idx in 0..MAX_TOOL_LOOPS {
        // Build full request body from current history.  Sanitize defensively
        // — drain or truncate may have left orphan tool turns at the boundary.
        let body = {
            let mut h = history.lock().await;
            sanitize_history(&mut h);
            build_body(&h, overrides.as_ref(), project_context.as_deref())
        };

        tracing::info!(loop = loop_idx, "calling gemini");
        let resp_text = match http_post(&client, &url, &body).await {
            Ok(t)  => t,
            Err(e) => return done(&app, &req_id, Some(e)),
        };

        let parsed: GeminiResponse = match serde_json::from_str(&resp_text) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(err = %e, body = %&resp_text[..resp_text.len().min(400)], "parse fail");
                return done(&app, &req_id, Some(format!("parse error: {e}")));
            }
        };

        if let Some(err) = parsed.error {
            return done(&app, &req_id, Some(err.message));
        }

        // Accumulate token usage from this iteration BEFORE consuming the
        // candidates field below.
        if let Some(u) = parsed.usage {
            total_input_tokens  += u.input_tokens();
            total_output_tokens += u.output_tokens();
        }

        let parts = parsed.candidates
            .and_then(|c| c.into_iter().next())
            .and_then(|c| c.content)
            .and_then(|c| c.parts)
            .unwrap_or_default();

        if parts.is_empty() {
            return done(&app, &req_id, Some("Gemini returned empty response.".into()));
        }

        // Split into function calls vs text vs thinking.
        let mut function_calls = Vec::new();
        let mut answer_text    = String::new();
        let mut thinking_text  = String::new();

        for part in parts {
            if let Some(fc) = part.function_call {
                function_calls.push(fc);
            } else if let Some(text) = part.text {
                if part.thought.unwrap_or(false) { thinking_text.push_str(&text); }
                else                              { answer_text.push_str(&text);   }
            }
        }

        if !function_calls.is_empty() {
            // Persist the model's tool-call turn into history (Gemini needs it).
            {
                let mut h = history.lock().await;
                h.push(ChatTurn::function_calls(&function_calls));
            }

            // Execute each tool, emitting one chunk per call with args+result.
            for fc in function_calls {
                let args_str = serde_json::to_string(&fc.args).unwrap_or_default();
                let result   = execute_tool(&tools, &fc.name, fc.args.clone()).await;
                let mut result_str = serde_json::to_string(&result).unwrap_or_default();
                if result_str.len() > 4000 {
                    result_str = format!("{}…(truncated, {} bytes)", &result_str[..4000], result_str.len());
                }
                let _ = app.emit("stream_chunk", StreamChunk {
                    id: req_id.clone(), text: String::new(),
                    thinking: None,
                    tool_call:   Some(fc.name.clone()),
                    tool_args:   Some(args_str),
                    tool_result: Some(result_str),
                    input_tokens: None, output_tokens: None,
                    done: false, error: None,
                });
                let mut h = history.lock().await;
                h.push(ChatTurn::function_response(&fc.name, &result));
            }
            continue; // back to step 2.
        }

        // No tool call — Gemini gave us a final text answer.
        if !thinking_text.is_empty() {
            let _ = app.emit("stream_chunk", StreamChunk {
                id: req_id.clone(), text: String::new(),
                thinking: Some(thinking_text),
                tool_call: None, tool_args: None, tool_result: None,
                input_tokens: None, output_tokens: None,
                done: false, error: None,
            });
        }

        let final_answer = if answer_text.trim().is_empty() {
            // Some Gemini responses put the entire reply in thought parts.
            "(no text)".to_string()
        } else {
            answer_text
        };

        // Persist the assistant's final answer.
        {
            let mut h = history.lock().await;
            h.push(ChatTurn { role: "model".into(), text: final_answer.clone() });
            persist(&h, &history_path);
        }

        // Stream word-by-word to the UI.
        for word in final_answer.split_inclusive(char::is_whitespace) {
            let _ = app.emit("stream_chunk", StreamChunk {
                id: req_id.clone(), text: word.to_string(),
                thinking: None,
                tool_call: None, tool_args: None, tool_result: None,
                input_tokens: None, output_tokens: None,
                done: false, error: None,
            });
            sleep(Duration::from_millis(15)).await;
        }

        return done_with_tokens(
            &app, &req_id, None,
            Some(total_input_tokens), Some(total_output_tokens),
        );
    }

    // Tool loop exhausted without a final answer — report the partial token
    // usage so the user can see the cost of the failed turn.
    done_with_tokens(
        &app, &req_id,
        Some(format!("Tool loop exceeded {MAX_TOOL_LOOPS} iterations.")),
        Some(total_input_tokens), Some(total_output_tokens),
    );
}

// ── Building the Gemini request ──────────────────────────────────────────────

fn build_body(
    history:         &[ChatTurn],
    overrides:       Option<&AgentOverrides>,
    project_context: Option<&str>,
) -> Value {
    // Convert ChatTurn back into Gemini Content objects.  Two special turn
    // shapes are encoded as JSON-in-text so we can round-trip across
    // restarts in the persisted chat.json:
    //   role="tool_call",     text="<json fc array>"     → role:"model"
    //   role="tool_response", text="<json {name,result}>" → role:"user"  (functionResponse)
    let mut contents: Vec<Value> = Vec::with_capacity(history.len());
    for turn in history {
        match turn.role.as_str() {
            "tool_call" => {
                let fcs: Vec<FunctionCall> = serde_json::from_str(&turn.text).unwrap_or_default();
                let parts: Vec<Value> = fcs.iter().map(|fc| {
                    json!({ "functionCall": { "name": fc.name, "args": fc.args } })
                }).collect();
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            "tool_response" => {
                #[derive(Deserialize)]
                struct TR { name: String, result: Value }
                if let Ok(tr) = serde_json::from_str::<TR>(&turn.text) {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{ "functionResponse": { "name": tr.name, "response": tr.result } }]
                    }));
                }
            }
            _ => {
                contents.push(json!({
                    "role":  &turn.role,
                    "parts": [{ "text": &turn.text }]
                }));
            }
        }
    }

    // Effective system prompt + tool surface.  Default agent → hardcoded
    // SYSTEM_PROMPT + every declaration; orchestra agent → composed prompt
    // + filtered declarations matching its `allowed_tools`.
    let base_system_prompt: &str = overrides
        .map(|o| o.system_prompt.as_str())
        .unwrap_or(SYSTEM_PROMPT);

    // Prepend project context (CLAUDE.md / README.md / curated overview).
    // Trimmed empty input → no injection.  Stable section header so
    // harness traces don't drift between runs that share the same project.
    let system_prompt: String = match project_context {
        Some(ctx) if !ctx.trim().is_empty() => format!(
            "{base_system_prompt}\n\n---\n\n# Active project context\n\n{ctx}"
        ),
        _ => base_system_prompt.to_string(),
    };

    let declarations: Vec<Value> = match overrides {
        Some(o) if !o.allowed_tools.is_empty() => function_declarations()
            .into_iter()
            .filter(|d| d.get("name")
                .and_then(|n| n.as_str())
                .map(|n| o.allowed_tools.contains(n))
                .unwrap_or(false))
            .collect(),
        _ => function_declarations(),
    };

    json!({
        "system_instruction": { "parts": [{ "text": system_prompt }] },
        "contents": contents,
        "tools":    [{ "function_declarations": declarations }]
    })
}

async fn http_post(client: &reqwest::Client, url: &str, body: &Value) -> Result<String, String> {
    let resp = client.post(url).json(body).send().await
        .map_err(|e| format!("network: {e}"))?;
    let status = resp.status();
    let text   = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("Gemini {status}: {}", &text[..text.len().min(500)]));
    }
    Ok(text)
}

// ── Tool execution ───────────────────────────────────────────────────────────

async fn execute_tool(
    tools: &std::collections::HashMap<String, Box<dyn NativeTool>>,
    name:  &str,
    args:  Value,
) -> Value {
    let tool = match tools.get(name) {
        Some(t) => t,
        None    => return json!({ "error": format!("unknown tool: {name}") }),
    };
    let call = ProtoToolCall { name: name.to_string(), args };
    match tool.run(call).await {
        Ok(r)  => serde_json::to_value(&r).unwrap_or_else(|e| json!({ "error": e.to_string() })),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

// ── History sanitizer ────────────────────────────────────────────────────────

/// Drop turns from the start of `history` until it begins with a valid first
/// turn for the Gemini chat API.
///
/// Gemini rejects requests where:
///   - the conversation does not start with a user turn (text or
///     `functionResponse`), or
///   - a `functionCall` appears without an immediately preceding user / tool
///     response turn.
///
/// Both situations can occur after `MAX_HISTORY_TURNS` drains the front of the
/// log mid-pair, or after `truncate_history` cuts at a boundary that leaves
/// orphan `tool_call` / `tool_response` rows behind.  We fix the prefix
/// in-place and also strip any trailing dangling `tool_call` (no matching
/// response) — that pair must always be complete.
pub fn sanitize_history(history: &mut Vec<ChatTurn>) {
    // 1. The first turn must be a `user` text turn.  Drop leading orphan
    //    `tool_call` (would be a model-functionCall) and `tool_response`
    //    (functionResponse with no preceding call), and any leading `model`
    //    turn (assistant text without a triggering user message).
    while let Some(first) = history.first() {
        match first.role.as_str() {
            "user" => break,
            _      => { history.remove(0); }
        }
    }

    // 2. Any `tool_call` must be immediately followed by its `tool_response`.
    //    If the loop was cancelled or persistence raced, the trailing entry
    //    may be a lone `tool_call`; drop it.
    while let Some(last) = history.last() {
        if last.role == "tool_call" {
            history.pop();
        } else {
            break;
        }
    }
}

// ── Persistence + emit helpers ───────────────────────────────────────────────

fn persist(history: &[ChatTurn], path: &std::path::Path) {
    if let Ok(json) = serde_json::to_string_pretty(history) {
        if let Err(e) = std::fs::write(path, json) {
            tracing::warn!(err = %e, "failed to persist chat history");
        }
    }
}

fn done(app: &AppHandle, id: &str, error: Option<String>) {
    done_with_tokens(app, id, error, None, None);
}

/// Variant of [`done`] that also reports the turn's aggregate token usage.
fn done_with_tokens(
    app:    &AppHandle,
    id:     &str,
    error:  Option<String>,
    input:  Option<u64>,
    output: Option<u64>,
) {
    let _ = app.emit("stream_chunk", StreamChunk {
        id: id.to_string(), text: String::new(),
        thinking: None,
        tool_call: None, tool_args: None, tool_result: None,
        done: true, error,
        input_tokens:  input,
        output_tokens: output,
    });
}

// ── Helpers on ChatTurn ──────────────────────────────────────────────────────

impl ChatTurn {
    fn function_calls(fcs: &[FunctionCall]) -> Self {
        let json = serde_json::to_string(fcs).unwrap_or_else(|_| "[]".to_string());
        Self { role: "tool_call".into(), text: json }
    }

    fn function_response(name: &str, result: &Value) -> Self {
        let json = serde_json::to_string(&json!({ "name": name, "result": result }))
            .unwrap_or_else(|_| "{}".to_string());
        Self { role: "tool_response".into(), text: json }
    }
}

// ── Gemini wire types ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<RespCandidate>>,
    error:      Option<RespError>,
    /// Per-request token accounting reported by Gemini.  Aggregated across
    /// every loop iteration so the final `StreamChunk` reflects the cost of
    /// the entire turn (chain of tool calls + final answer).
    #[serde(default, rename = "usageMetadata")]
    usage: Option<UsageMetadata>,
}

/// Maps `usageMetadata` from the Gemini v1beta `generateContent` response.
/// All fields are optional — older / preview models may omit them.
#[derive(Deserialize, Debug, Default, Clone, Copy)]
struct UsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    prompt_tokens:     u64,
    #[serde(default, rename = "candidatesTokenCount")]
    candidates_tokens: u64,
    /// Reasoning ("thinking") tokens for models that surface them separately
    /// (e.g. gemini-2.5-flash-thinking).  Always counted as output for cost.
    #[serde(default, rename = "thoughtsTokenCount")]
    thoughts_tokens:   u64,
    #[serde(default, rename = "totalTokenCount")]
    _total_tokens:     u64,
}

impl UsageMetadata {
    fn input_tokens(&self)  -> u64 { self.prompt_tokens }
    fn output_tokens(&self) -> u64 { self.candidates_tokens + self.thoughts_tokens }
}
#[derive(Deserialize)]
struct RespError { message: String }
#[derive(Deserialize)]
struct RespCandidate { content: Option<RespContent> }
#[derive(Deserialize)]
struct RespContent { parts: Option<Vec<RespPart>> }
#[derive(Deserialize)]
struct RespPart {
    #[serde(default)]                 text:          Option<String>,
    #[serde(default)]                 thought:       Option<bool>,
    #[serde(default, rename="functionCall")] function_call: Option<FunctionCall>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FunctionCall { name: String, #[serde(default)] args: Value }
