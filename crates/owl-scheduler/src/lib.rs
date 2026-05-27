//! Owl-Scheduler — cron-driven autonomous task runner.
//!
//! Mirrors Hermes' built-in cron: every [`ScheduledTask`] runs the agent
//! on a wall-clock schedule with its stored `prompt` as the input.  The
//! runner stays alive between fires (`Scheduler::run_forever`) or can be
//! ticked once for tests (`Scheduler::tick`).
//!
//! Storage is pluggable via `owl_protocol::schedule::SchedulerStore`;
//! the in-process [`InMemoryStore`] here covers tests, and
//! `owl_vault::SurrealSchedulerStore` covers production.

#![forbid(unsafe_code)]

pub mod runner;
pub mod store;

pub use runner::{Scheduler, SchedulerConfig};
pub use store::InMemoryStore;

use owl_protocol::schedule::ScheduledTask;

/// Construct a [`ScheduledTask`] with a fresh UUID v4 id.
///
/// Convenience wrapper for the common case where callers don't care
/// about supplying a specific id.
pub fn new_task(
    name:      impl Into<String>,
    cron_expr: impl Into<String>,
    prompt:    impl Into<String>,
) -> ScheduledTask {
    ScheduledTask::new(
        uuid::Uuid::new_v4().to_string(),
        name,
        cron_expr,
        prompt,
    )
}
