//! Cross-module sanity tests for the orchestra spec types.
//!
//! These tests exercise the full Serde round-trip + key invariants so we
//! catch breaking schema changes before they reach the loader.

#![cfg(test)]

use super::*;

fn agent_fixture() -> AgentSpec {
    AgentSpec {
        schema_version: SCHEMA_VERSION,
        id:           AgentId::new("researcher").unwrap(),
        name:         "Researcher".into(),
        description:  "Read-only code locator".into(),
        model:        Some(ModelSpec {
            provider: ProviderRef::Gemini,
            id:       ModelRef::new("gemini-2.5-flash"),
        }),
        system_prompt: "You are the Researcher.".into(),
        max_steps:     12,
        allowed_tools: vec![ToolName::new("read_file"), ToolName::new("grep")],
        skills:        vec![SkillId::new("grep_callsites").unwrap()],
        can_spawn:     false,
        max_depth:     0,
        max_input_tokens:  Some(50_000),
        max_output_tokens: Some(8_000),
    }
}

#[test]
fn agent_spec_round_trips_through_json() {
    let spec = agent_fixture();
    let json = serde_json::to_string_pretty(&spec).expect("serialise");
    let back: AgentSpec = serde_json::from_str(&json).expect("deserialise");
    // Compare a few stable fields — `schemars::JsonSchema` derives don't
    // include PartialEq, so we equality-check piece-by-piece.
    assert_eq!(back.id,            spec.id);
    assert_eq!(back.name,          spec.name);
    assert_eq!(back.allowed_tools, spec.allowed_tools);
    assert_eq!(back.skills,        spec.skills);
    assert_eq!(back.can_spawn,     spec.can_spawn);
}

#[test]
fn workflow_step_dependency_round_trips() {
    let wf = WorkflowSpec {
        schema_version: SCHEMA_VERSION,
        id:          WorkflowId::new("refactor").unwrap(),
        name:        "Refactor".into(),
        description: "test".into(),
        trigger:     Some("/refactor".into()),
        steps: vec![
            StepSpec {
                id:        StepId::new("plan").unwrap(),
                agent:     AgentId::new("researcher").unwrap(),
                depends:   vec![],
                prompt:    "Plan: {{user_input}}".into(),
                on_failure: None,
            },
            StepSpec {
                id:        StepId::new("code").unwrap(),
                agent:     AgentId::new("coder").unwrap(),
                depends:   vec![StepId::new("plan").unwrap()],
                prompt:    "Code: {{plan.output}}".into(),
                on_failure: Some(FailurePolicy::RetryOnce),
            },
        ],
        on_failure:        FailurePolicy::Abort,
        timeout_ms:        300_000,
        max_total_tokens:  Some(80_000),
    };

    let toml = toml::to_string(&wf).expect("toml encode");
    assert!(toml.contains("[[steps]]"));
    let back: WorkflowSpec = toml::from_str(&toml).expect("toml decode");
    assert_eq!(back.steps.len(), 2);
    assert_eq!(back.steps[1].depends[0].as_str(), "plan");
    assert_eq!(back.steps[1].on_failure, Some(FailurePolicy::RetryOnce));
}

#[test]
fn command_expansion_text_round_trips() {
    let cmd = CommandSpec {
        schema_version: SCHEMA_VERSION,
        id:          CommandId::new("test").unwrap(),
        name:        "test".into(),
        description: "Run cargo test".into(),
        expansion:   CommandExpansion::Text {
            template: "Run cargo test -p {{$1}} and report failures".into(),
        },
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains(r#""type":"text""#));
    let back: CommandSpec = serde_json::from_str(&json).unwrap();
    match back.expansion {
        CommandExpansion::Text { template } => {
            assert!(template.contains("{{$1}}"));
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn command_expansion_workflow_round_trips() {
    let cmd = CommandSpec {
        schema_version: SCHEMA_VERSION,
        id:          CommandId::new("audit").unwrap(),
        name:        "audit".into(),
        description: "".into(),
        expansion:   CommandExpansion::Workflow {
            workflow: WorkflowId::new("audit_security").unwrap(),
        },
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let back: CommandSpec = serde_json::from_str(&json).unwrap();
    match back.expansion {
        CommandExpansion::Workflow { workflow } => {
            assert_eq!(workflow.as_str(), "audit_security");
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn schema_version_constant_visible() {
    // Lock the constant so a silent bump is caught by review.
    assert_eq!(SCHEMA_VERSION, 1);
}

#[test]
fn spec_kind_displays_as_lowercase() {
    assert_eq!(SpecKind::Agent.to_string(),    "agent");
    assert_eq!(SpecKind::Workflow.to_string(), "workflow");
}

#[test]
fn unresolved_reference_error_carries_kind() {
    let e = OrchestraProtoError::UnresolvedReference {
        kind: "skill",
        id:   "missing".into(),
    };
    assert!(e.to_string().contains("skill"));
    assert!(e.to_string().contains("missing"));
}
