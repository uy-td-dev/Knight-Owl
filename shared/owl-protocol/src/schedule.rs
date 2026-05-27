//! Scheduled-task types — Hermes-style cron automation.
//!
//! `ScheduledTask` rows describe an autonomous agent invocation: a cron
//! expression + a prompt the agent runs whenever the cron fires.
//! Persistence is via `SchedulerStore` (lives in `owl-scheduler` so
//! `owl-protocol` stays storage-free per R-14).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A single scheduled agent invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// Stable UUID v4.
    pub id: String,
    /// Human-readable label for `owl schedule list`.
    pub name: String,
    /// Cron expression in the standard 6-field form
    /// `"sec min hour day month dow"` (e.g. `"0 0 9 * * Mon-Fri"`).
    pub cron_expr: String,
    /// Prompt fed into the agent every time the cron fires.
    pub prompt: String,
    /// Unix-epoch millis when the task last fired.  `None` for "never".
    pub last_run_ms: Option<i64>,
    /// `false` skips the task without deleting it — useful for pausing.
    pub enabled: bool,
}

impl ScheduledTask {
    /// Construct a new enabled task with no run history.  The caller
    /// supplies the id (typically a UUID v4) — owl-protocol stays
    /// dep-free per R-14.
    pub fn new(
        id:        impl Into<String>,
        name:      impl Into<String>,
        cron_expr: impl Into<String>,
        prompt:    impl Into<String>,
    ) -> Self {
        Self {
            id:          id.into(),
            name:        name.into(),
            cron_expr:   cron_expr.into(),
            prompt:      prompt.into(),
            last_run_ms: None,
            enabled:     true,
        }
    }
}

/// Errors returned by [`SchedulerStore`] operations.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum SchedulerError {
    #[error("scheduler store error: {0}")]
    Store(String),
    #[error("invalid cron expression: {0}")]
    Cron(String),
}

/// Pluggable persistence for [`ScheduledTask`]s.
///
/// Default impls are no-ops so backends without scheduling support
/// (e.g. in-memory test stores) keep compiling.  Concrete:
/// `owl_vault::SurrealSchedulerStore`.
#[async_trait]
pub trait SchedulerStore: Send + Sync {
    /// Insert or update a task by id.
    async fn upsert_task(&self, _t: ScheduledTask) -> Result<(), SchedulerError> { Ok(()) }

    /// List every task in arbitrary order — caller filters / sorts.
    async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, SchedulerError> { Ok(Vec::new()) }

    /// Delete a task by id.  Returns `Ok(())` even if it didn't exist.
    async fn delete_task(&self, _id: &str) -> Result<(), SchedulerError> { Ok(()) }

    /// Record that a task fired.  Called by the runner after every dispatch.
    async fn mark_fired(&self, _id: &str, _ts_ms: i64) -> Result<(), SchedulerError> { Ok(()) }
}
