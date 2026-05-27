//! Phase A — auto-skill generation by DistillationWorker.
//!
//! Feeds the worker 5 Success-outcome TaskMemory rows sharing a common
//! request shape, mocks the model to return a SkillDraft JSON, and
//! verifies that the SkillWriter receives the parsed draft.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use owl_brain::distillation::DistillationWorker;
use owl_brain::skill_writer::{SkillDraft, SkillWriter};
use owl_brain::BrainError;
use owl_harness::mock_engine::MockEngine;
use owl_protocol::experience::{
    ExperienceError, ExperienceStore, Insight, TaskMemory, TaskOutcome,
};

/// In-memory ExperienceStore that returns a fixed slate of task memories.
struct StaticExperience {
    memories: Vec<TaskMemory>,
    insights: Mutex<Vec<Insight>>,
}
#[async_trait]
impl ExperienceStore for StaticExperience {
    async fn store_task_memory(&self, _: TaskMemory) -> Result<(), ExperienceError> { Ok(()) }
    async fn recall_insights(&self, _: &str) -> Result<Vec<Insight>, ExperienceError> {
        Ok(Vec::new())
    }
    async fn upsert_insight(&self, i: Insight) -> Result<(), ExperienceError> {
        self.insights.lock().unwrap().push(i);
        Ok(())
    }
    async fn recent_task_memories(&self, _: u64) -> Result<Vec<TaskMemory>, ExperienceError> {
        Ok(self.memories.clone())
    }
}

/// Records every draft handed to it.
#[derive(Default)]
struct RecordingSkillWriter {
    drafts: Mutex<Vec<SkillDraft>>,
}
#[async_trait]
impl SkillWriter for RecordingSkillWriter {
    async fn write_skill(&self, d: SkillDraft) -> Result<String, BrainError> {
        let id = d.id.clone();
        self.drafts.lock().unwrap().push(d);
        Ok(id)
    }
}

fn success_task(idx: usize) -> TaskMemory {
    TaskMemory {
        id:        format!("task-{idx}"),
        request:   "run cargo check after editing a rust file".into(),
        actions:   vec!["tool:edit_file".into(), "tool:bash".into()],
        outcome:   TaskOutcome::Success,
        code_refs: vec![],
        session_id: String::new(),
        input_tokens: 0,
        output_tokens: 0,
    }
}

#[tokio::test]
async fn distillation_writes_skill_for_high_success_cluster() {
    let memories: Vec<TaskMemory> = (0..6).map(success_task).collect();

    // Model returns: (1) an Insight JSON for the existing distillation
    // step, then (2) a SkillDraft JSON for the new skill step.
    let model = MockEngine::new()
        .on("",
            r###"{"kind":"Pattern","scope":"global","summary":"Run cargo check after every Rust edit"}"###)
        .on("",
            r###"{"name":"Run Cargo Check After Edits","description":"Verify each Rust edit compiles before claiming done.","trigger":"after editing a .rs file","recommended_tools":["bash","edit_file"],"body":"## When\nAfter every .rs edit.\n\n## Steps\n1. Run `cargo check`.\n2. Read stderr if it fails.\n3. Patch and retry."}"###);

    let exp = Arc::new(StaticExperience {
        memories,
        insights: Mutex::new(Vec::new()),
    });
    let writer = Arc::new(RecordingSkillWriter::default());

    let worker = DistillationWorker::new(
        model,
        Arc::clone(&exp) as Arc<dyn ExperienceStore>,
    )
    .with_skill_writer(Arc::clone(&writer) as Arc<dyn SkillWriter>);

    let n = worker.run_once(50).await.unwrap();
    assert!(n >= 1, "at least one artefact emitted (got {n})");

    // Insight written.
    assert_eq!(exp.insights.lock().unwrap().len(), 1);

    // Skill draft handed to the writer.
    let drafts = writer.drafts.lock().unwrap();
    assert_eq!(drafts.len(), 1, "exactly one skill minted");
    let d = &drafts[0];
    assert_eq!(d.name, "Run Cargo Check After Edits");
    assert!(d.id.starts_with("auto-"), "id stamped by worker");
    assert!(d.body.contains("## Steps"));
    assert_eq!(d.recommended_tools, vec!["bash".to_string(), "edit_file".to_string()]);
}

#[tokio::test]
async fn distillation_skips_skill_when_cluster_too_small() {
    // Only 3 successes — below SKILL_CLUSTER_SIZE (5).  Should still
    // produce an Insight but NO skill.
    let memories: Vec<TaskMemory> = (0..3).map(success_task).collect();

    let model = MockEngine::new()
        .on("",
            r#"{"kind":"Pattern","scope":"global","summary":"small pattern"}"#);

    let exp = Arc::new(StaticExperience {
        memories,
        insights: Mutex::new(Vec::new()),
    });
    let writer = Arc::new(RecordingSkillWriter::default());
    let worker = DistillationWorker::new(
        model,
        Arc::clone(&exp) as Arc<dyn ExperienceStore>,
    )
    .with_skill_writer(Arc::clone(&writer) as Arc<dyn SkillWriter>);

    worker.run_once(50).await.unwrap();
    assert_eq!(exp.insights.lock().unwrap().len(), 1, "insight still written");
    assert!(writer.drafts.lock().unwrap().is_empty(),
            "no skill emitted under threshold");
}
