//! Cron-tick runner.
//!
//! Holds a `SchedulerStore` + an `AgentRunner` and fires each task when
//! its cron expression's next-run time passes.  Designed to live as a
//! long-running tokio task (`run_forever`); tests use `tick` directly
//! with an explicit `now` timestamp for determinism.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cron::Schedule;
use tracing::{info, warn};

use owl_brain::AgentRunner;
use owl_protocol::schedule::{ScheduledTask, SchedulerError, SchedulerStore};

/// Tunables for [`Scheduler`].
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// How long `run_forever` sleeps between ticks when no task is due
    /// soon.  Real production deployments can set this to 30-60s; tests
    /// run a single `tick()` and ignore this field.
    pub tick_interval: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self { tick_interval: Duration::from_secs(30) }
    }
}

/// Cron-driven invocation runner.
pub struct Scheduler {
    store:  Arc<dyn SchedulerStore>,
    runner: Arc<dyn AgentRunner>,
    config: SchedulerConfig,
}

impl Scheduler {
    pub fn new(
        store:  Arc<dyn SchedulerStore>,
        runner: Arc<dyn AgentRunner>,
    ) -> Self {
        Self { store, runner, config: SchedulerConfig::default() }
    }

    pub fn with_config(mut self, config: SchedulerConfig) -> Self {
        self.config = config;
        self
    }

    /// Single tick — evaluate every task once against `now` and fire
    /// those whose next-run time is in the past since `last_run_ms`.
    /// Returns the number of tasks fired.
    ///
    /// Test entry point.  Production code calls [`Self::run_forever`].
    pub async fn tick(&self, now: DateTime<Utc>) -> Result<usize, SchedulerError> {
        let tasks = self.store.list_tasks().await?;
        let mut fired = 0usize;

        for task in tasks {
            if !task.enabled {
                continue;
            }
            let due = match next_due_passed(&task, now) {
                Ok(b)  => b,
                Err(e) => {
                    warn!(task = %task.id, err = %e, "skipping task with bad cron");
                    continue;
                }
            };
            if !due {
                continue;
            }

            info!(task = %task.id, name = %task.name, "scheduler fire");
            // Best-effort: a runner error doesn't poison the scheduler.
            // We still record the firing so the next tick doesn't replay it.
            if let Err(e) = self.runner.run(&task.prompt).await {
                warn!(task = %task.id, err = %e, "scheduled task failed");
            }
            let ts = now.timestamp_millis();
            if let Err(e) = self.store.mark_fired(&task.id, ts).await {
                warn!(task = %task.id, err = %e, "mark_fired failed");
            }
            fired += 1;
        }

        Ok(fired)
    }

    /// Long-running entrypoint.  Sleeps `config.tick_interval` between
    /// ticks.  Returns only on cancellation (caller drops the future).
    pub async fn run_forever(&self) -> ! {
        loop {
            let now = Utc::now();
            if let Err(e) = self.tick(now).await {
                warn!(err = %e, "scheduler tick failed");
            }
            tokio::time::sleep(self.config.tick_interval).await;
        }
    }
}

/// `true` if `task`'s next-fire time after `last_run_ms` (or epoch when
/// never run) is ≤ `now`.
fn next_due_passed(task: &ScheduledTask, now: DateTime<Utc>) -> Result<bool, SchedulerError> {
    let schedule = Schedule::from_str(&task.cron_expr)
        .map_err(|e| SchedulerError::Cron(e.to_string()))?;
    let after = task
        .last_run_ms
        .and_then(|ms| DateTime::<Utc>::from_timestamp_millis(ms))
        .unwrap_or(DateTime::<Utc>::from_timestamp(0, 0).unwrap());
    Ok(schedule.after(&after).next().is_some_and(|t| t <= now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryStore;

    use std::sync::Mutex;

    use async_trait::async_trait;
    use owl_brain::BrainError;
    use owl_protocol::attachment::Attachment;

    /// Records every prompt the scheduler hands it.
    #[derive(Default)]
    struct RecordingRunner { fires: Mutex<Vec<String>> }
    #[async_trait]
    impl AgentRunner for RecordingRunner {
        async fn run(&self, prompt: &str) -> Result<String, BrainError> {
            self.fires.lock().unwrap().push(prompt.to_string());
            Ok("ok".into())
        }
        async fn run_with_attachments(
            &self, prompt: &str, _: &[Attachment],
        ) -> Result<String, BrainError> {
            self.run(prompt).await
        }
    }

    #[tokio::test]
    async fn fires_task_when_cron_due() {
        let store: Arc<dyn SchedulerStore> = Arc::new(InMemoryStore::new());
        // Every second — guaranteed due in the next tick.
        store.upsert_task(crate::new_task(
            "tick-test",
            "* * * * * *",
            "hello",
        )).await.unwrap();

        let runner_impl = Arc::new(RecordingRunner::default());
        let runner: Arc<dyn AgentRunner> = Arc::clone(&runner_impl) as Arc<dyn AgentRunner>;

        let sched = Scheduler::new(Arc::clone(&store), runner);
        // Use a `now` well after epoch so cron's after() definitely yields a slot.
        let now = Utc::now();
        let fired = sched.tick(now).await.unwrap();
        assert_eq!(fired, 1);
        assert_eq!(runner_impl.fires.lock().unwrap().len(), 1);

        // last_run_ms recorded — next tick within the same second must NOT refire.
        let tasks = store.list_tasks().await.unwrap();
        assert!(tasks[0].last_run_ms.is_some());
    }

    #[tokio::test]
    async fn skips_disabled_tasks() {
        let store: Arc<dyn SchedulerStore> = Arc::new(InMemoryStore::new());
        let mut t = crate::new_task("off", "* * * * * *", "hi");
        t.enabled = false;
        store.upsert_task(t).await.unwrap();

        let runner_impl = Arc::new(RecordingRunner::default());
        let runner: Arc<dyn AgentRunner> = Arc::clone(&runner_impl) as Arc<dyn AgentRunner>;
        let sched = Scheduler::new(store, runner);
        let fired = sched.tick(Utc::now()).await.unwrap();
        assert_eq!(fired, 0);
        assert!(runner_impl.fires.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn skips_invalid_cron_without_panicking() {
        let store: Arc<dyn SchedulerStore> = Arc::new(InMemoryStore::new());
        store.upsert_task(crate::new_task(
            "bad",
            "not a cron expr",
            "x",
        )).await.unwrap();

        let runner: Arc<dyn AgentRunner> = Arc::new(RecordingRunner::default());
        let sched = Scheduler::new(store, runner);
        let fired = sched.tick(Utc::now()).await.unwrap();
        assert_eq!(fired, 0, "invalid cron is logged + skipped");
    }
}
