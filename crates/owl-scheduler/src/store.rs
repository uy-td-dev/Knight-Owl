//! In-memory `SchedulerStore` for tests + ephemeral deployments.

use std::sync::Mutex;

use async_trait::async_trait;

use owl_protocol::schedule::{ScheduledTask, SchedulerError, SchedulerStore};

/// Tasks held in a `Mutex<Vec<_>>` — safe to clone the `Arc` and share
/// across the scheduler runner + CLI manipulation.
#[derive(Default)]
pub struct InMemoryStore {
    tasks: Mutex<Vec<ScheduledTask>>,
}

impl InMemoryStore {
    pub fn new() -> Self { Self::default() }
}

#[async_trait]
impl SchedulerStore for InMemoryStore {
    async fn upsert_task(&self, t: ScheduledTask) -> Result<(), SchedulerError> {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(slot) = tasks.iter_mut().find(|x| x.id == t.id) {
            *slot = t;
        } else {
            tasks.push(t);
        }
        Ok(())
    }

    async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, SchedulerError> {
        Ok(self.tasks.lock().unwrap().clone())
    }

    async fn delete_task(&self, id: &str) -> Result<(), SchedulerError> {
        self.tasks.lock().unwrap().retain(|t| t.id != id);
        Ok(())
    }

    async fn mark_fired(&self, id: &str, ts_ms: i64) -> Result<(), SchedulerError> {
        if let Some(t) = self.tasks.lock().unwrap().iter_mut().find(|x| x.id == id) {
            t.last_run_ms = Some(ts_ms);
        }
        Ok(())
    }
}
