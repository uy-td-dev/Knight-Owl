//! `SurrealSchedulerStore` — persistent `cron_task` rows backed by SurrealDB.
//!
//! Implements `owl_protocol::schedule::SchedulerStore` so the
//! `owl_scheduler::Scheduler` runner sees the same data the CLI's
//! `owl schedule add/list/remove` mutates.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use surrealdb::engine::any::Any;
use surrealdb::Surreal;

use owl_protocol::schedule::{ScheduledTask, SchedulerError, SchedulerStore};

use crate::HybridStore;

/// Surreal-backed scheduler store.
///
/// Wraps a `HybridStore` (typically `SurrealStore`) so the schema bootstrap
/// already in place covers our `cron_task` table.  The `Surreal<Any>`
/// handle is shared by cloning — both safe and cheap.
pub struct SurrealSchedulerStore {
    db: Surreal<Any>,
}

impl SurrealSchedulerStore {
    /// Construct from a connected SurrealStore.  Uses the same db handle
    /// so namespace/db selection done at connect time is honoured.
    pub fn new(store: Arc<crate::SurrealStore>) -> Self {
        Self { db: store.db().clone() }
    }
}

#[async_trait]
impl SchedulerStore for SurrealSchedulerStore {
    async fn upsert_task(&self, t: ScheduledTask) -> Result<(), SchedulerError> {
        self.db
            .query(
                "UPSERT type::record('cron_task', $id) CONTENT { \
                   id: $id, name: $name, cron_expr: $cron, prompt: $prompt, \
                   last_run_ms: $last, enabled: $enabled \
                 }",
            )
            .bind(json!({
                "id":      t.id,
                "name":    t.name,
                "cron":    t.cron_expr,
                "prompt":  t.prompt,
                "last":    t.last_run_ms,
                "enabled": t.enabled,
            }))
            .await
            .map_err(|e| SchedulerError::Store(e.to_string()))?
            .check()
            .map_err(|e| SchedulerError::Store(e.to_string()))?;
        Ok(())
    }

    async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, SchedulerError> {
        let mut resp = self.db
            .query(
                "SELECT meta::id(id) AS id, name, cron_expr, prompt, \
                        last_run_ms, enabled \
                 FROM cron_task ORDER BY name",
            )
            .await
            .map_err(|e| SchedulerError::Store(e.to_string()))?
            .check()
            .map_err(|e| SchedulerError::Store(e.to_string()))?;
        let rows: Vec<Value> = resp.take(0)
            .map_err(|e| SchedulerError::Store(e.to_string()))?;
        Ok(rows.into_iter().filter_map(row_to_task).collect())
    }

    async fn delete_task(&self, id: &str) -> Result<(), SchedulerError> {
        self.db
            .query("DELETE type::record('cron_task', $id)")
            .bind(json!({ "id": id }))
            .await
            .map_err(|e| SchedulerError::Store(e.to_string()))?
            .check()
            .map_err(|e| SchedulerError::Store(e.to_string()))?;
        Ok(())
    }

    async fn mark_fired(&self, id: &str, ts_ms: i64) -> Result<(), SchedulerError> {
        self.db
            .query(
                "UPDATE type::record('cron_task', $id) SET last_run_ms = $ts",
            )
            .bind(json!({ "id": id, "ts": ts_ms }))
            .await
            .map_err(|e| SchedulerError::Store(e.to_string()))?
            .check()
            .map_err(|e| SchedulerError::Store(e.to_string()))?;
        Ok(())
    }
}

fn row_to_task(v: Value) -> Option<ScheduledTask> {
    let obj = v.as_object()?;
    Some(ScheduledTask {
        id:          obj.get("id")?.as_str()?.to_string(),
        name:        obj.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
        cron_expr:   obj.get("cron_expr").and_then(Value::as_str).unwrap_or_default().to_string(),
        prompt:      obj.get("prompt").and_then(Value::as_str).unwrap_or_default().to_string(),
        last_run_ms: obj.get("last_run_ms").and_then(Value::as_i64),
        enabled:     obj.get("enabled").and_then(Value::as_bool).unwrap_or(true),
    })
}
