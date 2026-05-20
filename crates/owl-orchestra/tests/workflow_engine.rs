//! End-to-end test of `SequentialEngine` against an in-memory registry +
//! a mock `AgentFactory` whose runners just echo their prompt.
//!
//! Verifies that:
//! - Steps execute in topological order
//! - `{{user_input}}` and `{{step.output}}` substitutions reach the runner
//! - WorkflowEvent stream contains the expected sequence
//! - `Continue` policy keeps independent steps running after a failure
//! - Cancellation aborts the run before the next step

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use owl_brain::{AgentFactory, AgentRunner, BrainError};
use owl_orchestra::{
    InMemoryRegistry, Registry, SequentialEngine, WorkflowEngine, WorkflowEvent, WorkflowInput,
};
use owl_protocol::orchestra::{
    AgentId, AgentSpec, FailurePolicy, ModelRef, ModelSpec, ProviderRef, SkillSpec, StepId,
    StepSpec, TraceId, WorkflowId, WorkflowSpec, SCHEMA_VERSION,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ─── Mocks ───────────────────────────────────────────────────────────────────

/// Echoes the prompt back, optionally tagged so step outputs are
/// distinguishable in assertions.
struct EchoRunner { tag: String }

#[async_trait]
impl AgentRunner for EchoRunner {
    async fn run(&self, prompt: &str) -> Result<String, BrainError> {
        Ok(format!("[{}] {}", self.tag, prompt))
    }
}

/// Always returns an `EchoRunner` tagged with the agent id.
struct EchoFactory;

#[async_trait]
impl AgentFactory for EchoFactory {
    async fn build(
        &self,
        spec:    Arc<AgentSpec>,
        _skills: Vec<Arc<SkillSpec>>,
        _depth:  u8,
    ) -> Result<Arc<dyn AgentRunner>, BrainError> {
        Ok(Arc::new(EchoRunner { tag: spec.id.0.clone() }))
    }
}

/// Factory that returns failures for an allowlist of agent ids — used to
/// exercise retry / continue policies.
struct FailingFactory {
    fail_for: Vec<String>,
    fail_count: Arc<Mutex<HashMap<String, u32>>>,
    /// If `Some(n)`, the agent fails the first `n` attempts then succeeds.
    /// `None` → always fails.
    succeed_after: Option<u32>,
}

#[async_trait]
impl AgentFactory for FailingFactory {
    async fn build(
        &self,
        spec:    Arc<AgentSpec>,
        _skills: Vec<Arc<SkillSpec>>,
        _depth:  u8,
    ) -> Result<Arc<dyn AgentRunner>, BrainError> {
        let id = spec.id.0.clone();
        let succeed_after = self.succeed_after;
        let fail_for = self.fail_for.clone();
        let fail_count = Arc::clone(&self.fail_count);
        Ok(Arc::new(MaybeFailRunner { id, fail_for, fail_count, succeed_after }))
    }
}

struct MaybeFailRunner {
    id: String,
    fail_for: Vec<String>,
    fail_count: Arc<Mutex<HashMap<String, u32>>>,
    succeed_after: Option<u32>,
}

#[async_trait]
impl AgentRunner for MaybeFailRunner {
    async fn run(&self, prompt: &str) -> Result<String, BrainError> {
        if !self.fail_for.iter().any(|f| f == &self.id) {
            return Ok(format!("[{}] {}", self.id, prompt));
        }
        let mut counts = self.fail_count.lock().unwrap();
        let n = counts.entry(self.id.clone()).or_insert(0);
        *n += 1;
        let count = *n;
        drop(counts);
        match self.succeed_after {
            Some(threshold) if count > threshold =>
                Ok(format!("[{}] recovered: {}", self.id, prompt)),
            _ => Err(BrainError::ToolDispatch(format!("simulated failure #{count} for {}", self.id))),
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn agent_spec(id: &str) -> AgentSpec {
    AgentSpec {
        schema_version: SCHEMA_VERSION,
        id:           AgentId::new_reserved(id).unwrap(),
        name:         id.into(),
        description:  String::new(),
        model:        Some(ModelSpec {
            provider: ProviderRef::Inherit, id: ModelRef::new("test"),
        }),
        system_prompt: format!("system for {id}"),
        max_steps:     8,
        allowed_tools: vec![],
        skills:        vec![],
        can_spawn:     false,
        max_depth:     0,
        max_input_tokens:  None,
        max_output_tokens: None,
    }
}

fn step(id: &str, agent: &str, deps: &[&str], prompt: &str, on_fail: Option<FailurePolicy>) -> StepSpec {
    StepSpec {
        id:      StepId::new_reserved(id).unwrap(),
        agent:   AgentId::new_reserved(agent).unwrap(),
        depends: deps.iter().map(|d| StepId::new_reserved(*d).unwrap()).collect(),
        prompt:  prompt.into(),
        on_failure: on_fail,
    }
}

fn workflow(id: &str, steps: Vec<StepSpec>, on_fail: FailurePolicy) -> WorkflowSpec {
    WorkflowSpec {
        schema_version: SCHEMA_VERSION,
        id:           WorkflowId::new(id).unwrap(),
        name:         id.into(),
        description:  String::new(),
        trigger:      None,
        steps,
        on_failure:   on_fail,
        timeout_ms:   60_000,
        max_total_tokens: None,
    }
}

fn make_registry(specs: Vec<AgentSpec>) -> Arc<InMemoryRegistry> {
    let r = Arc::new(InMemoryRegistry::new());
    for s in specs {
        r.register_agent(s, PathBuf::from("/test")).expect("register agent");
    }
    r
}

fn collect_events(rx: &mut mpsc::Receiver<WorkflowEvent>) -> Vec<WorkflowEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() { out.push(e); }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn linear_pipeline_threads_outputs() {
    let registry = make_registry(vec![
        agent_spec("alpha"), agent_spec("beta"), agent_spec("gamma"),
    ]);
    let factory: Arc<dyn AgentFactory> = Arc::new(EchoFactory);

    let wf = workflow("demo", vec![
        step("a", "alpha", &[],    "input: {{user_input}}",       None),
        step("b", "beta",  &["a"], "from_a: {{a.output}}",        None),
        step("c", "gamma", &["b"], "from_b: {{b.output}}",        None),
    ], FailurePolicy::Abort);

    let engine = SequentialEngine::new(Arc::clone(&registry) as Arc<dyn Registry>, factory);
    let (tx, mut rx) = mpsc::channel(64);

    let outcome = engine.run(
        Arc::new(wf),
        WorkflowInput {
            trace_id:   TraceId::new("t1"),
            user_input: "hello".into(),
            seed:       None,
        },
        tx, CancellationToken::new(),
    ).await.expect("run ok");

    assert!(outcome.success);
    assert_eq!(outcome.step_results.len(), 3);
    let final_text = outcome.final_text;
    // The final step's prompt was: "from_b: [beta] from_a: [alpha] input: hello"
    assert!(final_text.contains("[gamma]"));
    assert!(final_text.contains("[beta]"));
    assert!(final_text.contains("[alpha]"));
    assert!(final_text.contains("input: hello"));

    // Event sequence: started → 3× (started, completed) → completed
    let events = collect_events(&mut rx);
    assert!(matches!(events.first(), Some(WorkflowEvent::WorkflowStarted { .. })));
    let n_started = events.iter().filter(|e| matches!(e, WorkflowEvent::StepStarted { .. })).count();
    let n_done    = events.iter().filter(|e| matches!(e, WorkflowEvent::StepCompleted { .. })).count();
    assert_eq!(n_started, 3);
    assert_eq!(n_done, 3);
    assert!(matches!(events.last(), Some(WorkflowEvent::WorkflowCompleted { .. })));
}

#[tokio::test]
async fn abort_policy_stops_on_first_failure() {
    let registry = make_registry(vec![agent_spec("flaky"), agent_spec("never_runs")]);
    let factory: Arc<dyn AgentFactory> = Arc::new(FailingFactory {
        fail_for: vec!["flaky".into()],
        fail_count: Arc::new(Mutex::new(HashMap::new())),
        succeed_after: None,
    });

    let wf = workflow("demo", vec![
        step("a", "flaky",      &[],    "anything", None),
        step("b", "never_runs", &["a"], "anything", None),
    ], FailurePolicy::Abort);

    let engine = SequentialEngine::new(registry, factory);
    let (tx, mut rx) = mpsc::channel(64);

    let err = engine.run(
        Arc::new(wf),
        WorkflowInput { trace_id: TraceId::new("t"), user_input: "x".into(), seed: None },
        tx, CancellationToken::new(),
    ).await.expect_err("must abort");

    use owl_orchestra::WorkflowError;
    assert!(matches!(err, WorkflowError::StepFailed { .. }));

    // The second step never started.
    let events = collect_events(&mut rx);
    let started_ids: Vec<_> = events.iter().filter_map(|e| match e {
        WorkflowEvent::StepStarted { step, .. } => Some(step.as_str().to_string()),
        _ => None,
    }).collect();
    assert_eq!(started_ids, vec!["a"]);
}

#[tokio::test]
async fn retry_once_recovers_on_second_attempt() {
    let registry = make_registry(vec![agent_spec("flaky")]);
    let factory: Arc<dyn AgentFactory> = Arc::new(FailingFactory {
        fail_for: vec!["flaky".into()],
        fail_count: Arc::new(Mutex::new(HashMap::new())),
        succeed_after: Some(1), // fail #1, succeed #2
    });

    let wf = workflow("demo", vec![
        step("a", "flaky", &[], "anything", Some(FailurePolicy::RetryOnce)),
    ], FailurePolicy::Abort);

    let engine = SequentialEngine::new(registry, factory);
    let (tx, _rx) = mpsc::channel(64);

    let outcome = engine.run(
        Arc::new(wf),
        WorkflowInput { trace_id: TraceId::new("t"), user_input: "x".into(), seed: None },
        tx, CancellationToken::new(),
    ).await.expect("retry should recover");

    assert!(outcome.success);
    assert_eq!(outcome.step_results[0].attempts, 2);
}

#[tokio::test]
async fn continue_policy_keeps_independent_branches() {
    let registry = make_registry(vec![
        agent_spec("flaky"), agent_spec("solo"), agent_spec("downstream"),
    ]);
    let factory: Arc<dyn AgentFactory> = Arc::new(FailingFactory {
        fail_for: vec!["flaky".into()],
        fail_count: Arc::new(Mutex::new(HashMap::new())),
        succeed_after: None,
    });

    let wf = workflow("demo", vec![
        step("a", "flaky",      &[],    "anything", None),
        step("b", "downstream", &["a"], "from_a: {{a.output}}", None),
        step("c", "solo",       &[],    "independent", None),
    ], FailurePolicy::Continue);

    let engine = SequentialEngine::new(registry, factory);
    let (tx, _rx) = mpsc::channel(128);

    let outcome = engine.run(
        Arc::new(wf),
        WorkflowInput { trace_id: TraceId::new("t"), user_input: "x".into(), seed: None },
        tx, CancellationToken::new(),
    ).await.expect("continue policy must not propagate the error");

    assert!(!outcome.success); // some step failed
    let by_id: HashMap<_, _> = outcome.step_results.iter()
        .map(|r| (r.step_id.as_str().to_string(), r.status.clone()))
        .collect();
    use owl_orchestra::StepStatus;
    assert_eq!(by_id.get("a"), Some(&StepStatus::Failed));
    assert_eq!(by_id.get("b"), Some(&StepStatus::Skipped));
    assert_eq!(by_id.get("c"), Some(&StepStatus::Completed));
}

#[tokio::test]
async fn cancellation_aborts_before_next_step() {
    let registry = make_registry(vec![agent_spec("alpha"), agent_spec("beta")]);
    let factory: Arc<dyn AgentFactory> = Arc::new(EchoFactory);

    let wf = workflow("demo", vec![
        step("a", "alpha", &[],    "x", None),
        step("b", "beta",  &["a"], "y: {{a.output}}", None),
    ], FailurePolicy::Abort);

    let engine = SequentialEngine::new(registry, factory);
    let (tx, _rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();

    // Cancel immediately so the very first cancellation check trips.
    cancel.cancel();
    let err = engine.run(
        Arc::new(wf),
        WorkflowInput { trace_id: TraceId::new("t"), user_input: "x".into(), seed: None },
        tx, cancel,
    ).await.expect_err("must be cancelled");

    use owl_orchestra::WorkflowError;
    assert!(matches!(err, WorkflowError::Cancelled));
}
