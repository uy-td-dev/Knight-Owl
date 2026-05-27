//! The Orchestrator — coordinates without knowing concrete tools or models.
//!
//! All dependencies are injected via trait objects; `owl-brain` never imports
//! `owl-tower`, `owl-armory`, or `owl-mcp` directly.

#![forbid(unsafe_code)]

pub mod approval;
pub mod compactor;
pub mod config;
pub mod context;
pub mod distillation;
pub mod error;
pub mod event_sink;
pub mod factory;
pub mod filtered_executor;
pub mod memory;
pub mod model_class;
pub mod prompt;
pub mod reasoning_loop;
pub mod reviewer;
pub mod runner;
pub mod skill_writer;

/// Default system prompt — delegates to `prompt::SYSTEM`.
pub const SYSTEM_PROMPT: &str = prompt::SYSTEM;

pub use approval::{ApprovalDecision, ApprovalGate, PermissiveGate};
pub use compactor::{Compactor, LlmCompactor, NoOpCompactor};
pub use event_sink::{EventSink, NullSink};
pub use model_class::ModelClass;
pub use config::Config as BrainConfig;
pub use context::ContextProvider;
pub use error::BrainError;
pub use factory::{AgentFactory, NullFactory};
pub use filtered_executor::FilteredExecutor;
pub use memory::{InMemoryStore, MemoryStore};
pub use owl_protocol::experience::ExperienceStore;
pub use distillation::DistillationWorker;
pub use reasoning_loop::{ReasoningConfig, ReasoningLoop};
pub use reviewer::{NoOpReviewer, Reviewer};
pub use runner::AgentRunner;
pub use skill_writer::{NoOpSkillWriter, SkillDraft, SkillWriter};
