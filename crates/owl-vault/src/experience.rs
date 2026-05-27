//! `SurrealExperienceStore` — L4 experience backend backed by SurrealDB.
//!
//! Implements [`owl_protocol::experience::ExperienceStore`].
//! Also provides a [`Distiller`] that clusters task memories into insights
//! when triggered (R-22 distillation job).

use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use owl_protocol::code::Violation;
use owl_protocol::experience::{
    ExperienceError, ExperienceStore, Insight, InsightKind, TaskMemory, TaskOutcome, TestRun,
};

use crate::{HybridStore, VaultError};

/// SurrealDB-backed implementation of [`ExperienceStore`].
///
/// Wraps any [`HybridStore`] to delegate the actual SurrealDB calls.
pub struct SurrealExperienceStore {
    store: Arc<dyn HybridStore>,
}

impl SurrealExperienceStore {
    /// Construct from any [`HybridStore`] (typically `SurrealStore`).
    pub fn new(store: Arc<dyn HybridStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ExperienceStore for SurrealExperienceStore {
    async fn store_task_memory(&self, memory: TaskMemory) -> Result<(), ExperienceError> {
        self.store
            .store_task_memory(memory)
            .await
            .map_err(|e| ExperienceError::Store(e.to_string()))
    }

    async fn recall_insights(&self, scope_prefix: &str) -> Result<Vec<Insight>, ExperienceError> {
        self.store
            .recall_insights(scope_prefix)
            .await
            .map_err(|e| ExperienceError::Store(e.to_string()))
    }

    async fn upsert_insight(&self, insight: Insight) -> Result<(), ExperienceError> {
        self.store
            .upsert_insight(insight)
            .await
            .map_err(|e| ExperienceError::Store(e.to_string()))
    }

    async fn recent_task_memories(&self, limit: u64) -> Result<Vec<TaskMemory>, ExperienceError> {
        self.store
            .recent_task_memories(limit)
            .await
            .map_err(|e| ExperienceError::Store(e.to_string()))
    }

    async fn clear_insights(&self) -> Result<u64, ExperienceError> {
        self.store
            .clear_insights()
            .await
            .map_err(|e| ExperienceError::Store(e.to_string()))
    }

    async fn store_test_run(&self, run: TestRun) -> Result<(), ExperienceError> {
        self.store
            .store_test_run(run)
            .await
            .map_err(|e| ExperienceError::Store(e.to_string()))
    }

    async fn test_runs_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<TestRun>, ExperienceError> {
        self.store
            .test_runs_for_task(task_id)
            .await
            .map_err(|e| ExperienceError::Store(e.to_string()))
    }

    async fn record_violation(&self, v: Violation) -> Result<(), ExperienceError> {
        self.store
            .record_violation(v)
            .await
            .map_err(|e| ExperienceError::Store(e.to_string()))
    }

    async fn violations_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<Violation>, ExperienceError> {
        self.store
            .violations_for_task(task_id)
            .await
            .map_err(|e| ExperienceError::Store(e.to_string()))
    }
}

/// Distillation job — clusters task memories into insights (R-22).
///
/// Call `Distiller::run()` periodically (e.g. triggered by a SurrealDB event
/// or a cron job).  Only writes `Insight` rows — never modifies `TaskMemory`.
pub struct Distiller {
    store: Arc<dyn ExperienceStore>,
}

impl Distiller {
    pub fn new(store: Arc<dyn ExperienceStore>) -> Self {
        Self { store }
    }

    /// Cluster recent task memories and emit new insights.
    ///
    /// Rules (R-22):
    /// - ≥ 3 failures with the same error prefix → `AntiPattern` insight.
    /// - ≥ 5 successes with the same action sequence prefix → `Pattern` insight.
    ///
    /// Returns the newly created `Insight` ids.
    pub async fn run(&self) -> Result<Vec<String>, VaultError> {
        let memories = self
            .store
            .recent_task_memories(200)
            .await
            .map_err(|e| VaultError::Surreal(e.to_string()))?;

        let mut new_ids: Vec<String> = Vec::new();

        // Cluster failures by error-message prefix (first 80 chars).
        let mut failure_clusters: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for m in &memories {
            if let TaskOutcome::Failure { reason, .. } = &m.outcome {
                let key = reason.chars().take(80).collect::<String>();
                failure_clusters.entry(key).or_default().push(m.id.clone());
            }
        }

        for (key, evidence) in failure_clusters {
            if evidence.len() >= 3 {
                let insight = Insight {
                    id:      uuid::Uuid::new_v4().to_string(),
                    kind:    InsightKind::AntiPattern,
                    scope:   "global".into(),
                    summary: format!("Recurring failure: {key}"),
                    evidence: evidence.into_iter().take(10).collect(),
                };
                debug!(id = %insight.id, "new anti-pattern insight");
                self.store
                    .upsert_insight(insight.clone())
                    .await
                    .map_err(|e| VaultError::Surreal(e.to_string()))?;
                new_ids.push(insight.id);
            }
        }

        // Cluster successes by first-action prefix.
        let mut success_clusters: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for m in &memories {
            if matches!(m.outcome, TaskOutcome::Success) {
                if let Some(first_action) = m.actions.first() {
                    let key = first_action.chars().take(60).collect::<String>();
                    success_clusters.entry(key).or_default().push(m.id.clone());
                }
            }
        }

        for (key, evidence) in success_clusters {
            if evidence.len() >= 5 {
                let insight = Insight {
                    id:      uuid::Uuid::new_v4().to_string(),
                    kind:    InsightKind::Pattern,
                    scope:   "global".into(),
                    summary: format!("Reliable pattern starting with: {key}"),
                    evidence: evidence.into_iter().take(10).collect(),
                };
                debug!(id = %insight.id, "new pattern insight");
                self.store
                    .upsert_insight(insight.clone())
                    .await
                    .map_err(|e| VaultError::Surreal(e.to_string()))?;
                new_ids.push(insight.id);
            }
        }

        Ok(new_ids)
    }
}

