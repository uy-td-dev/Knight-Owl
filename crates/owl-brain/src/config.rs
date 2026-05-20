//! Runtime configuration for owl-brain.
//!
//! Source priority (low → high): hardcoded defaults → env vars.

use serde::Deserialize;

/// Configuration for the reasoning loop.
///
/// Loaded from `config/owl-brain.toml` if present; fields can be overridden
/// with the `OWL_BRAIN_*` env vars documented below.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Maximum tool-call iterations before the loop gives up.
    /// Env: `OWL_BRAIN_MAX_STEPS`
    #[serde(default = "defaults::max_steps")]
    pub max_steps: usize,

    /// Number of memory entries included in each planning prompt.
    /// Env: `OWL_BRAIN_MEMORY_LIMIT`
    #[serde(default = "defaults::memory_context_limit")]
    pub memory_context_limit: usize,
}

impl Config {
    /// Load defaults then apply env overrides.
    pub fn load() -> Self {
        let path = "config/owl-brain.toml";
        let mut cfg: Config = if std::path::Path::new(path).exists() {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|raw| toml::from_str(&raw).ok())
                .unwrap_or_default()
        } else {
            Config::default()
        };

        if let Ok(v) = std::env::var("OWL_BRAIN_MAX_STEPS") {
            if let Ok(n) = v.parse() { cfg.max_steps = n; }
        }
        if let Ok(v) = std::env::var("OWL_BRAIN_MEMORY_LIMIT") {
            if let Ok(n) = v.parse() { cfg.memory_context_limit = n; }
        }
        cfg
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_steps: defaults::max_steps(),
            memory_context_limit: defaults::memory_context_limit(),
        }
    }
}

mod defaults {
    pub fn max_steps() -> usize { 10 }
    pub fn memory_context_limit() -> usize { 20 }
}
