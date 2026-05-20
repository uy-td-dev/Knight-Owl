//! Runtime configuration for owl-tower.
//!
//! Source priority (low → high): hardcoded defaults → `config/owl-tower.toml` → env vars.

use serde::Deserialize;

use crate::cache::CachePolicy;

/// Which LLM provider to use.
///
/// Default: `Anthropic`. Set via `OWL_TOWER_PROVIDER` env var or `config/owl-tower.toml`.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Anthropic,
    Gemini,
    Ollama,
}

/// Configuration for LLM provider adapters.
///
/// Loaded from `config/owl-tower.toml` if present; fields can be overridden
/// with the `OWL_TOWER_*` env vars documented below.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Active LLM provider. Env: `OWL_TOWER_PROVIDER` (`anthropic`/`gemini`/`ollama`).
    #[serde(default)]
    pub provider: Provider,

    /// Model identifier — meaning depends on the active provider.
    /// Anthropic default: `claude-sonnet-4-6`. Gemini default: `gemini-2.5-flash`.
    /// Env: `OWL_TOWER_MODEL`
    #[serde(default = "defaults::model")]
    pub model: String,

    /// Embedding model identifier (used when provider supports embeddings).
    /// Gemini default: `gemini-embedding-002`.
    /// Env: `OWL_TOWER_EMBEDDING_MODEL`
    #[serde(default = "defaults::embedding_model")]
    pub embedding_model: String,

    /// Anthropic API key. Reads `ANTHROPIC_API_KEY` if absent.
    /// Never committed to `config/owl-tower.toml` — env-only secret.
    #[serde(skip)]
    pub api_key: Option<String>,

    /// Google AI API key for Gemini. Reads `GEMINI_API_KEY` if absent.
    /// Never committed to `config/owl-tower.toml` — env-only secret.
    #[serde(skip)]
    pub gemini_api_key: Option<String>,

    /// Anthropic prompt caching policy.  Default: [`CachePolicy::Ephemeral`].
    ///
    /// Note: full activation requires `rig-core ≥ 0.13`.  See
    /// [`crate::cache`] for the current dormant-state behavior.
    /// Env: `OWL_TOWER_CACHE_POLICY` (`off` | `ephemeral`)
    #[serde(default)]
    pub cache_policy: CachePolicy,
}

impl Config {
    /// Load defaults → toml → env overrides.
    pub fn load() -> Self {
        let path = "config/owl-tower.toml";
        let mut cfg: Config = if std::path::Path::new(path).exists() {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|raw| toml::from_str(&raw).ok())
                .unwrap_or_default()
        } else {
            Config::default()
        };

        if let Ok(v) = std::env::var("OWL_TOWER_PROVIDER") {
            cfg.provider = match v.to_lowercase().as_str() {
                "gemini" => Provider::Gemini,
                "ollama" => Provider::Ollama,
                _        => Provider::Anthropic,
            };
        }
        if let Ok(v) = std::env::var("OWL_TOWER_MODEL")           { cfg.model = v; }
        if let Ok(v) = std::env::var("OWL_TOWER_EMBEDDING_MODEL")  { cfg.embedding_model = v; }
        if let Ok(v) = std::env::var("OWL_TOWER_CACHE_POLICY") {
            cfg.cache_policy = match v.to_lowercase().as_str() {
                "off"       => CachePolicy::Off,
                "ephemeral" => CachePolicy::Ephemeral,
                _           => CachePolicy::default(),
            };
        }
        cfg.api_key        = std::env::var("ANTHROPIC_API_KEY").ok();
        cfg.gemini_api_key = std::env::var("GEMINI_API_KEY").ok();
        cfg
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider:        Provider::default(),
            model:           defaults::model(),
            embedding_model: defaults::embedding_model(),
            api_key:         None,
            gemini_api_key:  None,
            cache_policy:    CachePolicy::default(),
        }
    }
}

mod defaults {
    pub fn model()           -> String { "claude-sonnet-4-6".into() }
    pub fn embedding_model() -> String { "gemini-embedding-002".into() }
}
