//! Orchestra — The Conductor.
//!
//! Loads, validates, and registers agents / skills / workflows / commands from
//! disk (workspace `<root>/.knight-owl/` overrides global `~/.knight-owl/`).
//! Composes effective system prompts; will host the workflow execution engine
//! in a later phase.
//!
//! Allowed deps (R-13 refined): `owl-protocol`, `owl-brain`, `async-trait`,
//! `thiserror`, `tokio`, `tokio-util`, `serde`, `serde_json`, `toml`, `notify`,
//! `tracing`.  Never imports `owl-tower`, `owl-armory`, `owl-vault`, or any
//! provider SDK.  The `owl-brain` dep is required by the workflow engine to
//! call `AgentFactory::build` and `AgentRunner::run`.
//!
//! Crate-level guarantees:
//! - All loader / registry methods are `async` and `Send + Sync`.
//! - Hot-reload (later phase) uses atomic [`std::sync::Arc`] swaps so
//!   in-flight executions keep their old spec.
//! - Failures are typed via [`OrchestraError`]; we never panic.

#![forbid(unsafe_code)]

pub mod compose;
pub mod error;
pub mod frontmatter;
pub mod loader;
pub mod path;
pub mod registry;
pub mod scan;
pub mod skill_writer;
pub mod watcher;
pub mod workflow;

pub use error::OrchestraError;
pub use loader::{FsLoader, InMemoryLoader, Loader};
pub use path::OrchestraRoots;
pub use registry::{InMemoryRegistry, Registry, RegistryEvent, RegistrySnapshot};
pub use scan::{scan_roots, ScanReport};
pub use skill_writer::FsSkillWriter;
pub use watcher::{OrchestraWatcher, WatchError};
pub use workflow::{
    SequentialEngine, StepResult, StepStatus, TokenUsage, WorkflowEngine, WorkflowError,
    WorkflowEvent, WorkflowInput, WorkflowOutcome,
};
