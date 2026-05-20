//! Workflow execution — runs a [`WorkflowSpec`] to completion, threading
//! results between steps and emitting structured progress events.
//!
//! The v1 engine is **sequential** ([`SequentialEngine`]): steps execute in
//! topological order of `depends`, one at a time.  The spec already carries
//! a DAG (`StepSpec::depends`) so a parallel engine can be plugged in later
//! without touching schema or callers.
//!
//! Module layout:
//! - [`engine`]    — sequential engine impl + the `WorkflowEngine` trait
//! - [`template`]  — `{{user_input}} / {{step.output}} / {{step.json.path}}` resolver
//! - [`event`]     — `WorkflowEvent`, `WorkflowOutcome`, `StepResult`
//! - [`error`]     — typed `WorkflowError`

pub mod engine;
pub mod error;
pub mod event;
pub mod template;

pub use engine::{SequentialEngine, WorkflowEngine, WorkflowInput};
pub use error::WorkflowError;
pub use event::{StepResult, StepStatus, TokenUsage, WorkflowEvent, WorkflowOutcome};
