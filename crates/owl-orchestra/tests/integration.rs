//! End-to-end integration tests against the on-disk fixture tree.
//!
//! Verifies the full chain:
//!     filesystem → FsLoader → Spec → Registry → compose
//!
//! Fixtures live under `tests/fixtures/` and mirror the canonical
//! `.knight-owl/` layout from the Design Spec.

use std::path::PathBuf;

use owl_orchestra::{
    compose::{compose_for_agent, compose_system_prompt},
    FsLoader, InMemoryRegistry, Loader, Registry,
};
use owl_protocol::orchestra::{
    AgentId, CommandExpansion, CommandId, FailurePolicy, SkillId, StepId, WorkflowId,
};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[tokio::test]
async fn loads_full_fixture_tree_into_registry() {
    let loader   = FsLoader;
    let registry = InMemoryRegistry::new();

    // Agent
    let agent_dir = fixtures().join("agents/researcher");
    let agent = loader.load_agent(&agent_dir).await.expect("load agent");
    assert_eq!(agent.id.as_str(), "researcher");
    assert_eq!(agent.name, "Researcher");
    assert_eq!(agent.allowed_tools.len(), 4);
    assert_eq!(agent.skills, vec![SkillId::new("grep_callsites").unwrap()]);
    assert!(agent.system_prompt.starts_with("You are the **Researcher**"));
    registry.register_agent(agent.clone(), agent_dir).expect("register agent");

    // Skill
    let skill_path = fixtures().join("skills/grep_callsites.md");
    let skill = loader.load_skill(&skill_path).await.expect("load skill");
    assert_eq!(skill.id.as_str(), "grep_callsites");
    assert!(skill.body.contains("grep -rn"));
    registry.register_skill(skill, skill_path).expect("register skill");

    // Workflow
    let wf_path = fixtures().join("workflows/refactor.toml");
    let wf = loader.load_workflow(&wf_path).await.expect("load workflow");
    assert_eq!(wf.id.as_str(), "refactor");
    assert_eq!(wf.steps.len(), 3);
    assert_eq!(wf.on_failure, FailurePolicy::Abort);
    assert_eq!(wf.steps[1].on_failure, Some(FailurePolicy::RetryOnce));
    assert_eq!(wf.steps[1].depends, vec![StepId::new_reserved("plan").unwrap()]);
    registry.register_workflow(wf, wf_path).expect("register workflow");

    // Command
    let cmd_path = fixtures().join("commands/audit.md");
    let cmd = loader.load_command(&cmd_path).await.expect("load command");
    assert_eq!(cmd.id.as_str(), "audit");
    match cmd.expansion {
        CommandExpansion::Workflow { ref workflow } => assert_eq!(workflow.as_str(), "audit_security"),
        ref other => panic!("unexpected expansion: {other:?}"),
    }
    registry.register_command(cmd, cmd_path).expect("register command");

    // Snapshot
    let snap = registry.snapshot();
    assert_eq!(snap.agents.len(),    1);
    assert_eq!(snap.skills.len(),    1);
    assert_eq!(snap.workflows.len(), 1);
    assert_eq!(snap.commands.len(),  1);
}

#[tokio::test]
async fn compose_resolves_skills_against_registry() {
    let loader   = FsLoader;
    let registry = InMemoryRegistry::new();

    let agent_dir  = fixtures().join("agents/researcher");
    let agent      = loader.load_agent(&agent_dir).await.expect("agent");
    let skill_path = fixtures().join("skills/grep_callsites.md");
    let skill      = loader.load_skill(&skill_path).await.expect("skill");
    registry.register_skill(skill.clone(), skill_path).expect("register skill");

    let (prompt, missing) = compose_for_agent(&agent, &registry);
    assert!(missing.is_empty(), "no skill should be missing: {missing:?}");
    assert!(prompt.starts_with("You are the **Researcher**"));
    assert!(prompt.contains("## Active skills"));
    assert!(prompt.contains("### Grep Callsites"));
    assert!(prompt.contains("grep -rn"));
}

#[tokio::test]
async fn compose_handles_missing_skill_gracefully() {
    let loader   = FsLoader;
    let registry = InMemoryRegistry::new();

    let agent_dir = fixtures().join("agents/researcher");
    let agent     = loader.load_agent(&agent_dir).await.expect("agent");
    // Note: we intentionally do NOT register the skill.

    let (prompt, missing) = compose_for_agent(&agent, &registry);
    assert_eq!(missing, vec!["grep_callsites".to_string()]);
    assert!(prompt.starts_with("You are the **Researcher**"));
    assert!(!prompt.contains("Active skills")); // section is omitted when no skills resolve
}

#[tokio::test]
async fn id_mismatch_rejected_by_loader() {
    use std::path::Path;

    // Use the in-memory loader to construct a directory whose folder name
    // disagrees with `identity.id` in the toml.  This isolates the check
    // without touching the real filesystem.
    let loader = owl_orchestra::InMemoryLoader::new();
    loader.put(
        "/fake/coder/agent.toml",
        r#"
schema_version = 1
[identity]
id          = "researcher"
name        = "Mismatch"
description = "Folder says coder, file says researcher."
[capabilities]
        "#.trim_start(),
    );
    loader.put("/fake/coder/system.md", "stub");
    let result = loader.load_agent(Path::new("/fake/coder")).await;
    assert!(matches!(result,
        Err(owl_orchestra::OrchestraError::IdMismatch { .. })
    ));
}

#[tokio::test]
async fn schema_too_new_is_rejected() {
    use std::path::Path;

    let loader = owl_orchestra::InMemoryLoader::new();
    loader.put(
        "/fake/future/agent.toml",
        r#"
schema_version = 999
[identity]
id          = "future"
name        = "Future"
description = "From a newer Knight-Owl."
[capabilities]
        "#.trim_start(),
    );
    loader.put("/fake/future/system.md", "stub");
    let err = loader.load_agent(Path::new("/fake/future")).await
        .expect_err("future schema must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("999"), "error should mention version: {msg}");
}

#[test]
fn compose_with_no_skills_is_identity() {
    use owl_protocol::orchestra::{
        AgentSpec, ModelRef, ModelSpec, ProviderRef, SCHEMA_VERSION,
    };
    let agent = AgentSpec {
        schema_version: SCHEMA_VERSION,
        id:           AgentId::new("alone").unwrap(),
        name:         "Alone".into(),
        description:  "no skills".into(),
        model:        Some(ModelSpec { provider: ProviderRef::Inherit, id: ModelRef::new("x") }),
        system_prompt: "Hello.".into(),
        max_steps:     12,
        allowed_tools: vec![],
        skills:        vec![],
        can_spawn:     false,
        max_depth:     0,
        max_input_tokens:  None,
        max_output_tokens: None,
    };
    assert_eq!(compose_system_prompt(&agent, &[]), "Hello.");
}

#[test]
fn unused_imports_smoke() {
    // Touch the IDs that aren't asserted-on directly so the test file's
    // imports stay honest if upstream renames anything.
    let _: AgentId    = AgentId::new("a").unwrap();
    let _: SkillId    = SkillId::new("a").unwrap();
    let _: WorkflowId = WorkflowId::new("a").unwrap();
    let _: CommandId  = CommandId::new("a").unwrap();
}
