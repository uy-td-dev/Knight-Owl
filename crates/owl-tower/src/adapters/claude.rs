//! Claude (Anthropic) provider adapter.
//!
//! Exposes only `ClaudeConfig` and `build_claude_model` — the underlying
//! Anthropic client is never `pub`.

use rig::client::CompletionClient;
use rig::providers::anthropic;

use crate::cache::CachePolicy;
use crate::config::Config;
use crate::TowerError;

/// Configuration for the Claude provider.
///
/// Use [`Config::load`] in production; construct manually in tests.
pub type ClaudeConfig = Config;

/// Build a `rig` completion model for the Claude provider.
///
/// Reads model id from `Config::model` and API key from `Config::api_key`
/// (populated via `ANTHROPIC_API_KEY` env var in `Config::load()`).
///
/// When `cfg.cache_policy` is [`CachePolicy::Ephemeral`] (the default),
/// `with_automatic_caching()` is enabled — Anthropic's API receives a
/// top-level `cache_control: { "type": "ephemeral" }` and automatically
/// caches the system prompt + conversation prefix (5-min TTL, ~10× cheaper
/// on cached tokens after the first request).
pub fn build_claude_model(
    cfg: ClaudeConfig,
) -> Result<anthropic::completion::CompletionModel, TowerError> {
    let key = cfg.api_key.clone()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
        .ok_or_else(|| TowerError::Config(
            "ANTHROPIC_API_KEY not set; set it or switch provider \
             (OWL_TOWER_PROVIDER=gemini)".into()
        ))?;
    let client = anthropic::Client::new(&key)
        .map_err(|e| TowerError::Config(format!("anthropic client: {e}")))?;
    let model = client.completion_model(&cfg.model);
    let model = match cfg.cache_policy {
        CachePolicy::Off       => model,
        CachePolicy::Ephemeral => model.with_automatic_caching(),
    };
    Ok(model)
}
