//! Sequential workflow engine.
//!
//! Topo-sort steps by `depends`, execute one at a time, thread outputs
//! through the template renderer, and emit a [`WorkflowEvent`] stream the
//! caller can route to UI / harness / disk.
//!
//! ## Why sequential first
//!
//! - 80% of useful pipelines are linear (researcher → coder → reviewer);
//!   the spec already supports DAG, so a parallel impl can swap in later.
//! - Sequential runs are deterministic — same inputs + same registry +
//!   same seed = same trace.  That's the harness contract (R-23).
//! - Failure handling is far simpler with a strictly-ordered run.
//!
//! ## Failure policies
//!
//! - `Abort`     — emit `StepFailed`, abandon the run, return `WorkflowError`.
//! - `RetryOnce` — try the same step a second time; success → continue,
//!                 second failure → escalate to abort.
//! - `Continue`  — mark failed, skip every downstream step that depends on
//!                 it (transitively), keep running independent steps.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use owl_brain::AgentFactory;
use owl_protocol::orchestra::{
    FailurePolicy, OrchestraProtoError, StepId, StepSpec, TraceId, WorkflowSpec,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::registry::Registry;
use crate::workflow::{
    error::WorkflowError,
    event::{StepResult, StepStatus, TokenUsage, WorkflowEvent, WorkflowOutcome},
    template,
};

// ─── Public types ────────────────────────────────────────────────────────────

/// Runtime parameters for a workflow run.
#[derive(Debug, Clone)]
pub struct WorkflowInput {
    /// Caller-supplied id used to correlate every emitted event.
    pub trace_id:   TraceId,
    /// Free-text input that triggered the workflow (slash-command tail or
    /// the user's chat message, depending on call site).
    pub user_input: String,
    /// Optional sampler seed forwarded to every agent run for deterministic
    /// replay.  `None` → providers use their default RNG.
    pub seed:       Option<u64>,
}

/// Trait for the engine itself — keeps the host loosely coupled to the
/// concrete `SequentialEngine` so harness can substitute a recorder.
#[async_trait]
pub trait WorkflowEngine: Send + Sync {
    async fn run(
        &self,
        workflow: Arc<WorkflowSpec>,
        input:    WorkflowInput,
        emit:     mpsc::Sender<WorkflowEvent>,
        cancel:   CancellationToken,
    ) -> Result<WorkflowOutcome, WorkflowError>;
}

// ─── SequentialEngine ────────────────────────────────────────────────────────

pub struct SequentialEngine {
    registry: Arc<dyn Registry>,
    factory:  Arc<dyn AgentFactory>,
}

impl SequentialEngine {
    pub fn new(registry: Arc<dyn Registry>, factory: Arc<dyn AgentFactory>) -> Self {
        Self { registry, factory }
    }
}

#[async_trait]
impl WorkflowEngine for SequentialEngine {
    async fn run(
        &self,
        workflow: Arc<WorkflowSpec>,
        input:    WorkflowInput,
        emit:     mpsc::Sender<WorkflowEvent>,
        cancel:   CancellationToken,
    ) -> Result<WorkflowOutcome, WorkflowError> {
        let started_at = now_ms();
        let _ = emit.send(WorkflowEvent::WorkflowStarted {
            trace_id: input.trace_id.clone(),
            workflow: workflow.id.clone(),
        }).await;

        // Order steps by dependency; cycles are already rejected at parse.
        let order = topo_sort(&workflow.steps)
            .map_err(|cycle| WorkflowError::Brain(owl_brain::BrainError::Config(
                format!("workflow cycle around step `{}`", cycle))))?;

        let mut outputs: HashMap<StepId, String> = HashMap::new();
        let mut step_results: Vec<StepResult>     = Vec::with_capacity(order.len());
        let mut failed_steps: HashSet<StepId>     = HashSet::new();
        let total_tokens: TokenUsage              = TokenUsage::default();

        for step in order {
            // Cancellation check before starting any expensive work.
            if cancel.is_cancelled() {
                let _ = emit.send(WorkflowEvent::WorkflowCancelled {
                    trace_id: input.trace_id.clone(),
                    workflow: workflow.id.clone(),
                }).await;
                return Err(WorkflowError::Cancelled);
            }

            // Skip if any upstream dep failed (under Continue policy).
            if step.depends.iter().any(|d| failed_steps.contains(d)) {
                let reason = "upstream step failed".to_string();
                let _ = emit.send(WorkflowEvent::StepSkipped {
                    trace_id: input.trace_id.clone(),
                    step:     step.id.clone(),
                    reason:   reason.clone(),
                }).await;
                step_results.push(StepResult {
                    step_id: step.id.clone(),
                    agent:   step.agent.clone(),
                    status:  StepStatus::Skipped,
                    output:  String::new(),
                    error:   Some(reason),
                    tokens:  TokenUsage::default(),
                    started_at: now_ms(), finished_at: now_ms(),
                    attempts: 0,
                });
                failed_steps.insert(step.id.clone());
                continue;
            }

            // Render template + run, with per-step retry policy.
            let policy = step.on_failure.unwrap_or(workflow.on_failure);
            let attempts_cap = match policy {
                FailurePolicy::RetryOnce => 2,
                _                         => 1,
            };

            let prompt = template::render(&step.prompt, &input.user_input, &outputs)
                .map_err(|e| WorkflowError::Template {
                    step: step.id.clone(),
                    message: e.to_string(),
                })?;

            let _ = emit.send(WorkflowEvent::StepStarted {
                trace_id: input.trace_id.clone(),
                step:     step.id.clone(),
                agent:    step.agent.clone(),
                prompt:   prompt.clone(),
            }).await;

            let started = now_ms();
            let mut last_error: Option<String> = None;
            let mut attempts: u32 = 0;
            let mut output: Option<String> = None;

            while attempts < attempts_cap {
                attempts += 1;
                if cancel.is_cancelled() { break; }

                match self.run_one_step(&step, &prompt).await {
                    Ok(text) => { output = Some(text); break; }
                    Err(e)   => {
                        last_error = Some(e.to_string());
                        warn!(step = %step.id, attempt = attempts, error = %e, "step failed");
                    }
                }
            }

            let finished = now_ms();
            match output {
                Some(text) => {
                    outputs.insert(step.id.clone(), text.clone());
                    let result = StepResult {
                        step_id: step.id.clone(),
                        agent:   step.agent.clone(),
                        status:  StepStatus::Completed,
                        output:  text,
                        error:   None,
                        tokens:  TokenUsage::default(),  // populated when budget tracking lands
                        started_at: started, finished_at: finished, attempts,
                    };
                    let _ = emit.send(WorkflowEvent::StepCompleted {
                        trace_id: input.trace_id.clone(),
                        step:     step.id.clone(),
                        result:   result.clone(),
                    }).await;
                    step_results.push(result);
                }
                None => {
                    let reason = last_error.clone().unwrap_or_else(|| "unknown".into());
                    let _ = emit.send(WorkflowEvent::StepFailed {
                        trace_id: input.trace_id.clone(),
                        step:     step.id.clone(),
                        error:    reason.clone(),
                    }).await;
                    step_results.push(StepResult {
                        step_id: step.id.clone(),
                        agent:   step.agent.clone(),
                        status:  StepStatus::Failed,
                        output:  String::new(),
                        error:   Some(reason.clone()),
                        tokens:  TokenUsage::default(),
                        started_at: started, finished_at: finished, attempts,
                    });
                    failed_steps.insert(step.id.clone());

                    if matches!(policy, FailurePolicy::Abort | FailurePolicy::RetryOnce) {
                        return Err(WorkflowError::StepFailed {
                            step:   step.id.clone(),
                            reason,
                        });
                    }
                    // Continue policy → keep running independent steps.
                }
            }
            let _ = total_tokens; // budget tracking placeholder
        }

        // Build outcome.  `final_text` = output of the last completed step
        // in declaration order — workflow authors typically design the last
        // step to be the user-facing summary.
        let final_text = workflow.steps.iter().rev()
            .find_map(|s| outputs.get(&s.id).cloned())
            .unwrap_or_default();

        let outcome = WorkflowOutcome {
            trace_id:    input.trace_id.clone(),
            workflow_id: workflow.id.clone(),
            final_text,
            step_results,
            tokens:      total_tokens,
            started_at,
            finished_at: now_ms(),
            success:     failed_steps.is_empty(),
        };
        info!(workflow = %workflow.id, success = outcome.success,
              ms = outcome.finished_at - outcome.started_at, "workflow finished");
        let _ = emit.send(WorkflowEvent::WorkflowCompleted { outcome: outcome.clone() }).await;
        Ok(outcome)
    }
}

impl SequentialEngine {
    /// Look up the agent in the registry, resolve skills, build a runner,
    /// and dispatch the (already templated) prompt to it.  All errors are
    /// bubbled up as [`WorkflowError`] so the engine loop can apply
    /// retry / abort policy.
    async fn run_one_step(
        &self,
        step:   &StepSpec,
        prompt: &str,
    ) -> Result<String, WorkflowError> {
        let spec = self.registry.agent(&step.agent)
            .ok_or_else(|| WorkflowError::AgentNotFound {
                step:  step.id.clone(),
                agent: step.agent.clone(),
            })?;
        // Resolve declared skills via the registry.
        let mut skills = Vec::with_capacity(spec.skills.len());
        for sid in &spec.skills {
            if let Some(s) = self.registry.skill(sid) { skills.push(s); }
        }
        // Rank skills by relevance to the step prompt — avoids dumping all
        // skills into the system prompt when only a few are relevant.
        let skills = crate::compose::rank_skills(&skills, prompt, 5);
        // Build a fresh runner for this step.  Engine is stateless; the host
        // can layer caching on top if rebuilding becomes hot.
        let runner = self.factory.build(spec.clone(), skills, /* depth */ 0).await?;
        let answer = runner.run(prompt).await?;
        Ok(answer)
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Topological sort using Kahn's algorithm.  Stable order: when multiple
/// candidates are ready we pick them in the original Vec order.  Determinism
/// matters for harness baselines (R-23).
fn topo_sort(steps: &[StepSpec]) -> Result<Vec<StepSpec>, String> {
    let n = steps.len();
    let mut by_id: HashMap<&StepId, &StepSpec>      = HashMap::with_capacity(n);
    let mut indeg:  HashMap<StepId, usize>          = HashMap::with_capacity(n);
    let mut deps:   HashMap<StepId, Vec<StepId>>    = HashMap::with_capacity(n);
    for s in steps {
        by_id.insert(&s.id, s);
        indeg.insert(s.id.clone(), s.depends.len());
        deps.insert(s.id.clone(), s.depends.clone());
    }
    let mut ready: Vec<StepId> = steps.iter()
        .filter(|s| s.depends.is_empty())
        .map(|s| s.id.clone())
        .collect();
    let mut out:   Vec<StepSpec> = Vec::with_capacity(n);

    while let Some(id) = ready.first().cloned() {
        ready.remove(0);
        let spec = by_id.get(&id)
            .copied()
            .cloned()
            .ok_or_else(|| OrchestraProtoError::Validation(
                format!("unknown step `{id}` during topo sort")
            ).to_string())?;
        out.push(spec);

        // Decrement indegree of every step that depended on `id`.
        for s in steps {
            if s.depends.iter().any(|d| d == &id) {
                if let Some(d) = indeg.get_mut(&s.id) {
                    *d = d.saturating_sub(1);
                    if *d == 0 && !out.iter().any(|x| x.id == s.id)
                        && !ready.iter().any(|r| r == &s.id) {
                        ready.push(s.id.clone());
                    }
                }
            }
        }
        let _ = deps; // (kept for future cycle diagnostics)
    }

    if out.len() != n {
        // Find one node still with in-degree > 0 → its dependents form a cycle.
        let stuck = indeg.iter().find(|(_, &d)| d > 0)
            .map(|(k, _)| k.0.clone())
            .unwrap_or_else(|| "<unknown>".to_string());
        return Err(stuck);
    }
    Ok(out)
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use owl_protocol::orchestra::{StepSpec, AgentId, FailurePolicy};

    fn step(id: &str, agent: &str, depends: &[&str]) -> StepSpec {
        StepSpec {
            id:      StepId::new_reserved(id).unwrap(),
            agent:   AgentId::new_reserved(agent).unwrap(),
            depends: depends.iter().map(|d| StepId::new_reserved(*d).unwrap()).collect(),
            prompt:  "test".into(),
            on_failure: None,
        }
    }

    #[test]
    fn topo_sort_linear_chain() {
        let steps = vec![
            step("a", "x", &[]),
            step("b", "x", &["a"]),
            step("c", "x", &["b"]),
        ];
        let out = topo_sort(&steps).unwrap();
        assert_eq!(out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
                   vec!["a", "b", "c"]);
    }

    #[test]
    fn topo_sort_diamond() {
        // a → b, a → c, both → d
        let steps = vec![
            step("a", "x", &[]),
            step("b", "x", &["a"]),
            step("c", "x", &["a"]),
            step("d", "x", &["b", "c"]),
        ];
        let out = topo_sort(&steps).unwrap();
        let names: Vec<_> = out.iter().map(|s| s.id.as_str()).collect();
        // a first, d last; b/c in either order.
        assert_eq!(names[0], "a");
        assert_eq!(names[3], "d");
        assert!((names[1] == "b" && names[2] == "c") || (names[1] == "c" && names[2] == "b"));
    }

    #[test]
    fn topo_sort_preserves_declaration_order_when_independent() {
        let steps = vec![
            step("first",  "x", &[]),
            step("second", "x", &[]),
            step("third",  "x", &[]),
        ];
        let out = topo_sort(&steps).unwrap();
        assert_eq!(out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
                   vec!["first", "second", "third"]);
    }

    /// FailurePolicy::default() == Abort sanity check (used implicitly above).
    #[test]
    fn default_failure_policy_is_abort() {
        assert_eq!(FailurePolicy::default(), FailurePolicy::Abort);
    }
}
