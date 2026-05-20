//! User-editable runtime settings (Gemini key, SurrealDB endpoint, etc.).
//!
//! Persisted at `~/.knight-owl/settings.json` — these are machine-level
//! secrets, NOT per-workspace.  Loaded once at startup; an update from the
//! UI requires an app restart to take effect (the LLM client and DB pool
//! are constructed once during `AppState::new`).
//!
//! Source priority for runtime config:
//!   `process env var` > `settings.json` > hard-coded default.
//!
//! That means a value baked into the user's shell wins — opening the UI lets
//! them fill in any field that isn't already set in their environment.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

/// All persistable user settings.  Every field is optional; missing fields
/// fall through to the env var, then to the crate default.
///
/// **Single source of truth** for runtime config.  Stored at
/// `~/.knight-owl/settings.json` (user-local, never committed).  Loaded
/// once at boot via [`apply_to_env`] which projects values into the
/// process environment so each crate's existing `Config::load()` picks
/// them up without refactoring.
///
/// Priority (highest wins):
///   1. Pre-existing env var (set in shell)
///   2. `settings.json` field
///   3. Crate default
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // ── API keys ──────────────────────────────────────────────────
    /// Google AI API key.  Env: `GEMINI_API_KEY`.
    pub gemini_api_key:         Option<String>,
    /// Anthropic key.  Env: `ANTHROPIC_API_KEY`.
    pub anthropic_api_key:      Option<String>,

    // ── Provider + model ──────────────────────────────────────────
    /// Active LLM provider: `anthropic` | `gemini` | `ollama`.  Env: `OWL_TOWER_PROVIDER`.
    pub provider:               Option<String>,
    /// Chat model id for active provider.  Env: `OWL_TOWER_MODEL`.
    pub model:                  Option<String>,
    /// Anthropic prompt-cache policy: `off` | `ephemeral`.  Env: `OWL_TOWER_CACHE_POLICY`.
    pub cache_policy:           Option<String>,

    // ── Gemini-specific ───────────────────────────────────────────
    /// Gemini chat model id, e.g. `gemini-2.5-flash`.  Env: `OWL_GEMINI_MODEL`.
    pub gemini_model:           Option<String>,
    /// Gemini embedding model id.  Env: `OWL_GEMINI_EMBEDDING_MODEL`.
    pub gemini_embedding_model: Option<String>,

    // ── Ollama-specific ───────────────────────────────────────────
    /// Ollama daemon URL.  Env: `OWL_OLLAMA_BASE_URL`.
    pub ollama_base_url:        Option<String>,
    /// Ollama embedding model (e.g. `nomic-embed-text`).  Env: `OWL_OLLAMA_EMBED_MODEL`.
    pub ollama_embed_model:     Option<String>,

    // ── SurrealDB vault ───────────────────────────────────────────
    /// SurrealDB endpoint, e.g. `ws://localhost:8000`.  Env: `OWL_VAULT_ENDPOINT`.
    pub surreal_endpoint:       Option<String>,
    /// SurrealDB namespace.  Env: `OWL_VAULT_NAMESPACE`.
    pub surreal_namespace:      Option<String>,
    /// SurrealDB database.  Env: `OWL_VAULT_DATABASE`.
    pub surreal_database:       Option<String>,
    /// SurrealDB root username.  Env: `OWL_VAULT_USER`.
    pub surreal_username:       Option<String>,
    /// SurrealDB root password.  Env: `OWL_VAULT_PASS`.
    pub surreal_password:       Option<String>,

    // ── Brain / reasoning loop ────────────────────────────────────
    /// Max tool-call iterations per turn.  Env: `OWL_BRAIN_MAX_STEPS`.
    pub brain_max_steps:            Option<u32>,
    /// Memory entries pulled into context per turn.  Env: `OWL_BRAIN_MEMORY_CONTEXT_LIMIT`.
    pub brain_memory_context_limit: Option<u32>,

    // ── Sandbox ───────────────────────────────────────────────────
    /// `1`/`0` — wrap `bash` tool with Sandbox runner.  Env: `OWL_SANDBOX_BASH`.
    pub sandbox_bash_enabled:   Option<bool>,
}

/// Settings file: `~/.knight-owl/settings.json`.
pub fn settings_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".knight-owl");
        p.push("settings.json");
        return p;
    }
    PathBuf::from(".knight-owl/settings.json")
}

/// Read settings from disk; returns `Default` if the file is missing.
pub fn read_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Apply settings to the process environment for any env var that isn't
/// already set.  Call this BEFORE `AppState::new()` so `Config::load()` in
/// the various crates sees the values.
///
/// `process env var > settings.json > default` — we never override a key
/// the user already exported in their shell.
pub fn apply_to_env(s: &Settings) {
    fn set_if_unset(key: &str, val: &str) {
        if std::env::var_os(key).is_none() && !val.is_empty() {
            std::env::set_var(key, val);
        }
    }

    // String fields → env mapping.  Add new fields here as Settings grows.
    let str_pairs: &[(&str, &Option<String>)] = &[
        // API keys
        ("GEMINI_API_KEY",             &s.gemini_api_key),
        ("ANTHROPIC_API_KEY",          &s.anthropic_api_key),
        // Provider + model
        ("OWL_TOWER_PROVIDER",         &s.provider),
        ("OWL_TOWER_MODEL",            &s.model),
        ("OWL_TOWER_CACHE_POLICY",     &s.cache_policy),
        // Gemini
        ("OWL_GEMINI_MODEL",           &s.gemini_model),
        ("OWL_GEMINI_EMBEDDING_MODEL", &s.gemini_embedding_model),
        // Ollama
        ("OWL_OLLAMA_BASE_URL",        &s.ollama_base_url),
        ("OWL_OLLAMA_EMBED_MODEL",     &s.ollama_embed_model),
        // Vault
        ("OWL_VAULT_ENDPOINT",         &s.surreal_endpoint),
        ("OWL_VAULT_NAMESPACE",        &s.surreal_namespace),
        ("OWL_VAULT_DATABASE",         &s.surreal_database),
        ("OWL_VAULT_USER",             &s.surreal_username),
        ("OWL_VAULT_PASS",             &s.surreal_password),
    ];
    for (env, value) in str_pairs {
        if let Some(v) = value { set_if_unset(env, v); }
    }

    // Numeric / bool fields — stringify before setting.
    if let Some(v) = s.brain_max_steps            { set_if_unset("OWL_BRAIN_MAX_STEPS",            &v.to_string()); }
    if let Some(v) = s.brain_memory_context_limit { set_if_unset("OWL_BRAIN_MEMORY_CONTEXT_LIMIT", &v.to_string()); }
    if let Some(v) = s.sandbox_bash_enabled       { set_if_unset("OWL_SANDBOX_BASH", if v { "1" } else { "0" }); }
}

// ── Tauri commands ───────────────────────────────────────────────────────────

/// Returned to the UI: the raw on-disk settings + a parallel object showing
/// which keys are *currently* satisfied by the process environment.  The UI
/// uses the latter to badge fields as "from env" (read-only) vs "from file"
/// (editable).
#[derive(Debug, Serialize)]
pub struct SettingsView {
    pub settings: Settings,
    pub env_set:  EnvFlags,
    pub path:     String,
}

#[derive(Debug, Default, Serialize)]
pub struct EnvFlags {
    pub gemini_api_key:             bool,
    pub anthropic_api_key:          bool,
    pub provider:                   bool,
    pub model:                      bool,
    pub cache_policy:               bool,
    pub gemini_model:               bool,
    pub gemini_embedding_model:     bool,
    pub ollama_base_url:            bool,
    pub ollama_embed_model:         bool,
    pub surreal_endpoint:           bool,
    pub surreal_namespace:          bool,
    pub surreal_database:           bool,
    pub surreal_username:           bool,
    pub surreal_password:           bool,
    pub brain_max_steps:            bool,
    pub brain_memory_context_limit: bool,
    pub sandbox_bash_enabled:       bool,
}

#[tauri::command]
pub async fn get_settings(_state: State<'_, AppState>) -> Result<SettingsView, String> {
    Ok(SettingsView {
        settings: read_settings(),
        env_set:  EnvFlags {
            gemini_api_key:             std::env::var("GEMINI_API_KEY").is_ok(),
            anthropic_api_key:          std::env::var("ANTHROPIC_API_KEY").is_ok(),
            provider:                   std::env::var("OWL_TOWER_PROVIDER").is_ok(),
            model:                      std::env::var("OWL_TOWER_MODEL").is_ok(),
            cache_policy:               std::env::var("OWL_TOWER_CACHE_POLICY").is_ok(),
            gemini_model:               std::env::var("OWL_GEMINI_MODEL").is_ok(),
            gemini_embedding_model:     std::env::var("OWL_GEMINI_EMBEDDING_MODEL").is_ok(),
            ollama_base_url:            std::env::var("OWL_OLLAMA_BASE_URL").is_ok(),
            ollama_embed_model:         std::env::var("OWL_OLLAMA_EMBED_MODEL").is_ok(),
            surreal_endpoint:           std::env::var("OWL_VAULT_ENDPOINT").is_ok(),
            surreal_namespace:          std::env::var("OWL_VAULT_NAMESPACE").is_ok(),
            surreal_database:           std::env::var("OWL_VAULT_DATABASE").is_ok(),
            surreal_username:           std::env::var("OWL_VAULT_USER").is_ok(),
            surreal_password:           std::env::var("OWL_VAULT_PASS").is_ok(),
            brain_max_steps:            std::env::var("OWL_BRAIN_MAX_STEPS").is_ok(),
            brain_memory_context_limit: std::env::var("OWL_BRAIN_MEMORY_CONTEXT_LIMIT").is_ok(),
            sandbox_bash_enabled:       std::env::var("OWL_SANDBOX_BASH").is_ok(),
        },
        path: settings_path().display().to_string(),
    })
}

#[tauri::command]
pub async fn save_settings(
    settings: Settings,
    _state:   State<'_, AppState>,
) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    tracing::info!(path = %path.display(), "saved settings");
    Ok(())
}
