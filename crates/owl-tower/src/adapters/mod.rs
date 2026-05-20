//! Provider adapter modules, each gated by a feature flag.

#[cfg(feature = "claude")]
pub mod claude;

#[cfg(feature = "gemini")]
pub mod gemini;

#[cfg(feature = "ollama")]
pub mod ollama;
