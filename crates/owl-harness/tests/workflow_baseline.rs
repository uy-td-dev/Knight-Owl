//! Regression test — runs a fixed 3-step workflow against an in-memory
//! registry + a deterministic echo factory, records the trace, and diffs
//! it against `baselines/three_step_pipeline.json`.
//!
//! Update the baseline by deleting the JSON file and re-running this
//! test once with `OWL_HARNESS_RECORD=1`.  CI fails on any structural
//! change (event order, status flips, retry counts) so behaviour shifts
//! must be reviewed in PR.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use owl_brain::{AgentFactory, AgentRunner, BrainError};
use owl_harness::Trace;
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

/// Deterministic runner — output depends only on the prompt, no clock or
/// random state involved, so traces are byte-identical across runs.
struct EchoRunner { tag: String }

#[async_trait]
impl AgentRunner for EchoRunner {
    async fn run(&self, prompt: &str) -> Result<String, BrainError> {
        Ok(format!("[{}] {}", self.tag, prompt))
    }
}

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

// ─── Fixtures ────────────────────────────────────────────────────────────────

fn agent(id: &str) -> AgentSpec {
    AgentSpec {
        schema_version: SCHEMA_VERSION,
        id:           AgentId::new_reserved(id).unwrap(),
        name:         id.into(),
        description:  String::new(),
        model:        Some(ModelSpec {
            provider: ProviderRef::Inherit, id: ModelRef::new("test"),
        }),
        system_prompt: String::new(),
        max_steps:     8,
        allowed_tools: vec![],
        skills:        vec![],
        can_spawn:     false,
        max_depth:     0,
        max_input_tokens:  None,
        max_output_tokens: None,
    }
}

fn step(id: &str, agent_id: &str, deps: &[&str], prompt: &str) -> StepSpec {
    StepSpec {
        id:      StepId::new_reserved(id).unwrap(),
        agent:   AgentId::new_reserved(agent_id).unwrap(),
        depends: deps.iter().map(|d| StepId::new_reserved(*d).unwrap()).collect(),
        prompt:  prompt.into(),
        on_failure: None,
    }
}

fn make_workflow() -> WorkflowSpec {
    WorkflowSpec {
        schema_version: SCHEMA_VERSION,
        id:           WorkflowId::new("baseline").unwrap(),
        name:         "Baseline".into(),
        description:  String::new(),
        trigger:      None,
        steps: vec![
            step("plan",   "researcher", &[],        "plan: {{user_input}}"),
            step("code",   "coder",      &["plan"],  "code from {{plan.output}}"),
            step("verify", "reviewer",   &["code"],  "verify {{code.output}}"),
        ],
        on_failure:   FailurePolicy::Abort,
        timeout_ms:   60_000,
        max_total_tokens: None,
    }
}

fn make_registry() -> Arc<InMemoryRegistry> {
    let r = Arc::new(InMemoryRegistry::new());
    for id in &["researcher", "coder", "reviewer"] {
        r.register_agent(agent(id), PathBuf::from("/test")).expect("register");
    }
    r
}

// ─── The test ────────────────────────────────────────────────────────────────

const BASELINE_PATH: &str = "baselines/three_step_pipeline.json";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_step_pipeline_matches_baseline() {
    let registry: Arc<dyn Registry>           = make_registry();
    let factory:  Arc<dyn AgentFactory>       = Arc::new(EchoFactory);
    let engine = SequentialEngine::new(registry, factory);

    let (tx, rx) = mpsc::channel::<WorkflowEvent>(64);
    let recorder = tokio::spawn(Trace::record("three_step_pipeline", rx));

    engine.run(
        Arc::new(make_workflow()),
        WorkflowInput {
            trace_id:   TraceId::new("baseline-trace"),
            user_input: "extract Foo".into(),
            seed:       Some(42),
        },
        tx, CancellationToken::new(),
    ).await.expect("run ok");

    let trace = recorder.await.expect("recorder task ok");

    // Resolve the baseline path relative to the harness crate root.
    let baseline_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BASELINE_PATH);

    // Recording mode — `OWL_HARNESS_RECORD=1 cargo test` writes the
    // current trace as the new baseline.  Useful when adding or
    // intentionally changing behaviour.
    if std::env::var("OWL_HARNESS_RECORD").is_ok() {
        std::fs::create_dir_all(baseline_path.parent().unwrap()).ok();
        std::fs::write(&baseline_path, trace.to_json().unwrap())
            .expect("write baseline");
        println!("recorded baseline at {}", baseline_path.display());
        return;
    }

    // Normal mode — compare against the committed baseline.
    let baseline_raw = std::fs::read_to_string(&baseline_path)
        .unwrap_or_else(|e| panic!(
            "missing baseline at {}: {}\n\
             Run with OWL_HARNESS_RECORD=1 to create it.",
            baseline_path.display(), e
        ));
    let baseline = Trace::from_json(&baseline_raw).expect("parse baseline");

    let diffs = trace.diff(&baseline);
    if !diffs.is_empty() {
        let msg: Vec<String> = diffs.iter().map(ToString::to_string).collect();
        panic!(
            "Trace diverged from baseline ({} mismatch{}):\n\n{}\n\n\
             If this change is intentional, re-record with:\n\
             OWL_HARNESS_RECORD=1 cargo test -p owl-harness three_step_pipeline_matches_baseline",
            diffs.len(), if diffs.len() == 1 { "" } else { "es" },
            msg.join("\n"),
        );
    }
}
