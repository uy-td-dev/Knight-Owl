//! Runtime configuration for owl-cortex.
//!
//! Source priority (low → high): `config/owl-cortex.toml` → env vars.

use serde::Deserialize;

use crate::CortexError;

/// Configuration for the cortex ingestor.
///
/// Loaded from `config/owl-cortex.toml`; individual fields can be overridden
/// with the `OWL_CORTEX_*` env vars documented below.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Max bytes of source text kept as the `preview` field on each `CodeNode`.
    /// Env: `OWL_CORTEX_PREVIEW_BYTES`
    #[serde(default = "defaults::preview_bytes")]
    pub preview_bytes: usize,

    /// Languages to attempt parsing.
    /// Env: `OWL_CORTEX_LANGS` (comma-separated, e.g. `"rust,typescript"`)
    #[serde(default = "defaults::langs")]
    pub langs: Vec<String>,
}

impl Config {
    /// Load from `config/owl-cortex.toml` then apply env overrides.
    pub fn load() -> Result<Self, CortexError> {
        let path = "config/owl-cortex.toml";
        let mut cfg: Config = if std::path::Path::new(path).exists() {
            let raw = std::fs::read_to_string(path)?;
            toml::from_str(&raw).map_err(|e| CortexError::Config(e.to_string()))?
        } else {
            Config::default()
        };

        if let Ok(v) = std::env::var("OWL_CORTEX_PREVIEW_BYTES") {
            cfg.preview_bytes =
                v.parse().map_err(|_| CortexError::Config("OWL_CORTEX_PREVIEW_BYTES must be a number".into()))?;
        }
        if let Ok(v) = std::env::var("OWL_CORTEX_LANGS") {
            cfg.langs = v.split(',').map(str::trim).map(String::from).collect();
        }

        Ok(cfg)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            preview_bytes: defaults::preview_bytes(),
            langs: defaults::langs(),
        }
    }
}

mod defaults {
    pub fn preview_bytes() -> usize { 200 }
    pub fn langs() -> Vec<String> { vec!["rust".into()] }
}
