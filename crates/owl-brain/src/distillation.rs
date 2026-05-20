//! Distillation worker — clusters task memories and synthesizes insights.
//!
//! After every N completed tasks, the worker:
//! 1. Fetches recent `TaskMemory` rows from L4.
//! 2. Groups them by similarity (request text overlap).
//! 3. For each cluster of ≥3 members, calls the LLM to distill one `Insight`.
//! 4. Persists the insight via `ExperienceStore::upsert_insight`.

use std::sync::Arc;

use rig::agent::AgentBuilder;
use rig::completion::Prompt;
use tracing::{info, warn};

use owl_protocol::experience::{
    ExperienceStore, Insight, InsightKind, TaskMemory, TaskOutcome,
};

use crate::BrainError;

const MIN_CLUSTER_SIZE: usize = 3;

pub struct DistillationWorker<M>
where
    M: rig::completion::CompletionModel + Clone,
{
    model: M,
    experience: Arc<dyn ExperienceStore>,
}

impl<M> DistillationWorker<M>
where
    M: rig::completion::CompletionModel + Clone + Send + Sync + 'static,
{
    pub fn new(model: M, experience: Arc<dyn ExperienceStore>) -> Self {
        Self { model, experience }
    }

    pub async fn run_once(&self, batch_size: u64) -> Result<usize, BrainError> {
        let memories = self
            .experience
            .recent_task_memories(batch_size)
            .await
            .map_err(|e| BrainError::Memory(e.to_string()))?;

        if memories.len() < MIN_CLUSTER_SIZE {
            info!(count = memories.len(), "not enough task memories for distillation");
            return Ok(0);
        }

        let clusters = cluster_by_request(&memories);
        let mut insights_created = 0usize;

        for cluster in clusters {
            if cluster.len() < MIN_CLUSTER_SIZE {
                continue;
            }

            match self.distill_cluster(&cluster).await {
                Ok(insight) => {
                    let evidence: Vec<String> = cluster.iter().map(|m| m.id.clone()).collect();
                    let full_insight = Insight {
                        id: uuid::Uuid::new_v4().to_string(),
                        kind: insight.kind,
                        scope: insight.scope,
                        summary: insight.summary,
                        evidence,
                    };

                    if let Err(e) = self.experience.upsert_insight(full_insight).await {
                        warn!(err = %e, "failed to persist insight");
                    } else {
                        insights_created += 1;
                    }
                }
                Err(e) => {
                    warn!(err = %e, "distillation failed for cluster");
                }
            }
        }

        info!(insights_created, "distillation complete");
        Ok(insights_created)
    }

    async fn distill_cluster(&self, cluster: &[&TaskMemory]) -> Result<ParsedInsight, BrainError> {
        let prompt_text = format_cluster_prompt(cluster);
        let agent = AgentBuilder::new(self.model.clone())
            .preamble(crate::prompt::DISTILLATION_SYSTEM)
            .build();

        let raw = agent
            .prompt(prompt_text.as_str())
            .await
            .map_err(|e| BrainError::Completion(e.to_string()))?;

        parse_insight_response(&raw)
    }
}

#[derive(serde::Deserialize)]
struct ParsedInsight {
    kind: InsightKind,
    scope: String,
    summary: String,
}

fn format_cluster_prompt(cluster: &[&TaskMemory]) -> String {
    let mut parts = Vec::new();
    for (i, m) in cluster.iter().enumerate() {
        let outcome_str = match &m.outcome {
            TaskOutcome::Success => "SUCCESS".to_string(),
            TaskOutcome::Failure { reason, .. } => format!("FAILURE: {reason}"),
        };
        parts.push(format!(
            "Task {}: request=\"{}\" actions=[{}] outcome={} code_refs=[{}]",
            i + 1,
            m.request,
            m.actions.join(", "),
            outcome_str,
            m.code_refs.join(", "),
        ));
    }
    parts.join("\n")
}

fn parse_insight_response(raw: &str) -> Result<ParsedInsight, BrainError> {
    let start = raw.find('{').ok_or_else(|| {
        BrainError::Completion("no JSON object in distillation response".into())
    })?;
    let mut depth = 0i32;
    let mut end = start;
    for (i, ch) in raw[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    serde_json::from_str::<ParsedInsight>(&raw[start..end]).map_err(Into::into)
}

fn cluster_by_request<'a>(memories: &'a [TaskMemory]) -> Vec<Vec<&'a TaskMemory>> {
    let mut clusters: Vec<Vec<&TaskMemory>> = Vec::new();

    for mem in memories {
        let keywords: Vec<&str> = mem
            .request
            .split_whitespace()
            .filter(|w| w.len() >= 4)
            .collect();

        let mut best_cluster = None;
        let mut best_score = 0usize;

        for (idx, cluster) in clusters.iter().enumerate() {
            let representative = &cluster[0].request;
            let rep_keywords: Vec<&str> = representative
                .split_whitespace()
                .filter(|w| w.len() >= 4)
                .collect();
            let overlap = keywords
                .iter()
                .filter(|k| rep_keywords.contains(k))
                .count();
            if overlap > best_score && overlap >= 2 {
                best_score = overlap;
                best_cluster = Some(idx);
            }
        }

        if let Some(idx) = best_cluster {
            clusters[idx].push(mem);
        } else {
            clusters.push(vec![mem]);
        }
    }

    clusters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_groups_similar_requests() {
        let mems = vec![
            make_mem("fix the payment handler bug"),
            make_mem("fix the payment handler error"),
            make_mem("fix the payment handler crash"),
            make_mem("add logging to auth module"),
            make_mem("something completely different"),
        ];
        let clusters = cluster_by_request(&mems);
        let big = clusters.iter().filter(|c| c.len() >= 3).count();
        assert!(big >= 1, "expected at least one cluster of 3+");
    }

    #[test]
    fn parse_valid_insight_json() {
        let raw = r#"Here is the insight: {"kind":"AntiPattern","scope":"global","summary":"Avoid direct DB calls in handler"}"#;
        let ins = parse_insight_response(raw).unwrap();
        assert_eq!(ins.summary, "Avoid direct DB calls in handler");
    }

    fn make_mem(request: &str) -> TaskMemory {
        TaskMemory {
            id: uuid::Uuid::new_v4().to_string(),
            request: request.to_string(),
            actions: vec![],
            outcome: TaskOutcome::Success,
            code_refs: vec![],
            session_id: String::new(),
        }
    }
}
