//! The Watchtower — normalizes all LLM providers behind `rig` traits.
//!
//! Provider SDK types are never `pub`; only config, builders, and rig traits are exposed.
//! Feature flags: `claude` (default), `gemini`, `ollama`.

#![forbid(unsafe_code)]

pub mod adapters;
pub mod cache;
pub mod config;
pub mod error;
pub mod prompt;
pub mod vision;

pub use cache::{CachePolicy, PromptCache};
pub use error::TowerError;
