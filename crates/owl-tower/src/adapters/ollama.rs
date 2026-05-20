//! Ollama provider adapter (local LLM runner).
//!
//! Exposes only `OllamaConfig` and `build_ollama_model` — the underlying
//! Ollama client is never `pub`.  Ollama runs locally so no API key is
//! required; only the base URL (default `http://localhost:11434`) and
//! the model id need to be configured.

use rig::client::{CompletionClient, EmbeddingsClient, Nothing};
use rig::providers::ollama;

use crate::TowerError;

/// Configuration for the Ollama provider.
///
/// Load from env via [`OllamaConfig::load`]; construct manually in tests.
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// Ollama model identifier, e.g. `"llama3.2"` or `"qwen2.5-coder"`.
    /// Env: `OWL_OLLAMA_MODEL`
    pub model: String,

    /// Base URL of the Ollama HTTP server.
    /// Default: `http://localhost:11434`.
    /// Env: `OWL_OLLAMA_BASE_URL`
    pub base_url: String,
}

impl OllamaConfig {
    /// Load defaults → env overrides.
    pub fn load() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("OWL_OLLAMA_MODEL")    { cfg.model    = v; }
        if let Ok(v) = std::env::var("OWL_OLLAMA_BASE_URL") { cfg.base_url = v; }
        cfg
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            model:    "llama3.2".into(),
            base_url: "http://localhost:11434".into(),
        }
    }
}

/// Build a `rig` completion model backed by a local Ollama server.
///
/// Returns an error only if the model id is empty.  Connectivity to the
/// Ollama server is not validated until the first request.
pub fn build_ollama_model(
    cfg: OllamaConfig,
) -> Result<ollama::CompletionModel, TowerError> {
    if cfg.model.trim().is_empty() {
        return Err(TowerError::Config(
            "OWL_OLLAMA_MODEL is empty; set it to e.g. `llama3.2` or `qwen2.5-coder`".into(),
        ));
    }
    // Set OLLAMA_API_BASE_URL via env if user supplied a custom base; rig 0.36
    // reads it from there.
    if cfg.base_url != "http://localhost:11434" {
        std::env::set_var("OLLAMA_API_BASE_URL", &cfg.base_url);
    }
    let client = ollama::Client::new(Nothing)
        .map_err(|e| TowerError::Config(format!("ollama client: {e}")))?;
    Ok(client.completion_model(&cfg.model))
}

/// Build a `rig` embedding model backed by a local Ollama server.
///
/// Defaults to `nomic-embed-text` (768 dims) — the most popular open
/// embedder.  Override via `OWL_OLLAMA_EMBED_MODEL` env var, or by
/// supplying `model_id` directly.  The user must `ollama pull` the model
/// first.
pub fn build_ollama_embedder(
    base_url: &str,
    model_id: Option<&str>,
) -> Result<ollama::EmbeddingModel, TowerError> {
    let model = model_id.map(str::to_string)
        .or_else(|| std::env::var("OWL_OLLAMA_EMBED_MODEL").ok())
        .unwrap_or_else(|| "nomic-embed-text".to_string());
    if base_url != "http://localhost:11434" {
        std::env::set_var("OLLAMA_API_BASE_URL", base_url);
    }
    let client = ollama::Client::new(Nothing)
        .map_err(|e| TowerError::Config(format!("ollama client: {e}")))?;
    // `nomic-embed-text` outputs 768 dims; other embedders vary.  Conservative
    // default — the embedder API queries the model for actual dim count.
    let ndims = match model.as_str() {
        "nomic-embed-text"             => 768,
        "mxbai-embed-large"            => 1024,
        "all-minilm"                   => 384,
        "snowflake-arctic-embed:33m"   => 384,
        "snowflake-arctic-embed:110m"  => 768,
        "snowflake-arctic-embed"       => 1024,
        _                              => 768,  // safe default
    };
    Ok(client.embedding_model_with_ndims(&model, ndims))
}
