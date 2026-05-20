//! Google Gemini provider adapter.
//!
//! Exposes only `GeminiConfig`, `build_gemini_model`, and `build_gemini_embedder` —
//! the underlying Gemini client is never `pub`.

use rig::client::{CompletionClient, EmbeddingsClient};
use rig::providers::gemini;

use crate::TowerError;

/// Configuration for the Gemini provider.
///
/// Load from env via [`GeminiConfig::load`]; construct manually in tests.
#[derive(Debug, Clone)]
pub struct GeminiConfig {
    /// Gemini model identifier, e.g. `"gemini-2.5-flash"`.
    /// Env: `OWL_GEMINI_MODEL`
    pub model: String,

    /// Gemini embedding model identifier, e.g. `"gemini-embedding-002"`.
    /// Env: `OWL_GEMINI_EMBEDDING_MODEL`
    pub embedding_model: String,

    /// Google AI API key. Read from `GEMINI_API_KEY` — never committed to files.
    pub api_key: Option<String>,
}

impl GeminiConfig {
    /// Load defaults → env overrides.
    ///
    /// API key is read exclusively from `GEMINI_API_KEY`.
    pub fn load() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("OWL_GEMINI_MODEL") { cfg.model = v; }
        if let Ok(v) = std::env::var("OWL_GEMINI_EMBEDDING_MODEL") { cfg.embedding_model = v; }
        cfg.api_key = std::env::var("GEMINI_API_KEY").ok();
        cfg
    }
}

impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            model:           "gemini-2.5-flash".into(),
            embedding_model: "gemini-embedding-002".into(),
            api_key:         None,
        }
    }
}

/// Build a `rig` completion model backed by Google Gemini.
///
/// Uses [`GeminiConfig::model`] as the model id and reads the API key from
/// [`GeminiConfig::api_key`] (set via `GEMINI_API_KEY` in [`GeminiConfig::load`]).
pub fn build_gemini_model(
    cfg: GeminiConfig,
) -> Result<gemini::completion::CompletionModel, TowerError> {
    let key = cfg.api_key.clone()
        .or_else(|| std::env::var("GEMINI_API_KEY").ok())
        .ok_or_else(|| TowerError::Config(
            "GEMINI_API_KEY not set; set it in the environment that launched the app".into()
        ))?;
    let client = gemini::Client::new(&key)
        .map_err(|e| TowerError::Config(format!("gemini client: {e}")))?;
    Ok(client.completion_model(&cfg.model))
}

/// Build a `rig` embedding model backed by Google Gemini.
///
/// Uses [`GeminiConfig::embedding_model`] and reads the API key from
/// [`GeminiConfig::api_key`] (set via `GEMINI_API_KEY`).
/// `ndims` is set to 3072 to match `gemini-embedding-002`'s output dimension.
pub fn build_gemini_embedder(
    cfg: GeminiConfig,
) -> Result<gemini::embedding::EmbeddingModel, TowerError> {
    let key = cfg.api_key.clone()
        .or_else(|| std::env::var("GEMINI_API_KEY").ok())
        .ok_or_else(|| TowerError::Config(
            "GEMINI_API_KEY not set; set it in the environment that launched the app".into()
        ))?;
    let client = gemini::Client::new(&key)
        .map_err(|e| TowerError::Config(format!("gemini client: {e}")))?;
    // gemini-embedding-002 outputs 3072 dims by default.
    Ok(client.embedding_model_with_ndims(&cfg.embedding_model, 3072))
}
