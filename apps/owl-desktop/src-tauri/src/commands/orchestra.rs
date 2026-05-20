//! Read-only Tauri commands surfacing the orchestra registry to the UI.
//!
//! These are *list* commands only — the UI Agents page renders them but
//! doesn't yet mutate state.  Mutation (create / edit / delete) lands in a
//! later phase via `save_*` commands; for now the user edits files in their
//! editor and the watcher picks up changes automatically.

use serde::Serialize;
use tauri::State;

use owl_orchestra::Registry;

use crate::state::AppState;

// ─── DTOs (kept thin — UI-friendly subset of the protocol types) ─────────────

#[derive(Debug, Serialize)]
pub struct AgentListItem {
    pub id:            String,
    pub name:          String,
    pub description:   String,
    pub allowed_tools: Vec<String>,
    pub skills:        Vec<String>,
    pub can_spawn:     bool,
    pub max_steps:     usize,
    pub max_depth:     u8,
}

#[derive(Debug, Serialize)]
pub struct SkillListItem {
    pub id:                String,
    pub name:              String,
    pub description:       String,
    pub trigger:           Option<String>,
    pub recommended_tools: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowListItem {
    pub id:          String,
    pub name:        String,
    pub description: String,
    pub trigger:     Option<String>,
    pub steps:       Vec<WorkflowStepItem>,
    pub on_failure:  String,
    pub timeout_ms:  u64,
}

#[derive(Debug, Serialize)]
pub struct WorkflowStepItem {
    pub id:      String,
    pub agent:   String,
    pub depends: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CommandListItem {
    pub id:          String,
    pub name:        String,
    pub description: String,
    /// Discriminant only — full payload is loaded on demand if the UI ever
    /// needs to show command bodies.  Keeps `/`-menu autocomplete cheap.
    pub kind:        &'static str, // "text" | "workflow" | "tool"
    /// For workflow commands, the workflow id to run.
    pub workflow_id: Option<String>,
}

// ─── Commands ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_orchestra_agents(state: State<'_, AppState>) -> Result<Vec<AgentListItem>, String> {
    let mut items: Vec<AgentListItem> = state.orchestra.list_agents().into_iter()
        .map(|a| AgentListItem {
            id:            a.id.0.clone(),
            name:          a.name.clone(),
            description:   a.description.clone(),
            allowed_tools: a.allowed_tools.iter().map(|t| t.0.clone()).collect(),
            skills:        a.skills.iter().map(|s| s.0.clone()).collect(),
            can_spawn:     a.can_spawn,
            max_steps:     a.max_steps,
            max_depth:     a.max_depth,
        })
        .collect();
    // Stable sort — UI lists update predictably under hot-reload.
    items.sort_by(|x, y| x.id.cmp(&y.id));
    Ok(items)
}

#[tauri::command]
pub async fn list_orchestra_skills(state: State<'_, AppState>) -> Result<Vec<SkillListItem>, String> {
    let mut items: Vec<SkillListItem> = state.orchestra.list_skills().into_iter()
        .map(|s| SkillListItem {
            id:                s.id.0.clone(),
            name:              s.name.clone(),
            description:       s.description.clone(),
            trigger:           s.trigger.clone(),
            recommended_tools: s.recommended_tools.iter().map(|t| t.0.clone()).collect(),
        })
        .collect();
    items.sort_by(|x, y| x.id.cmp(&y.id));
    Ok(items)
}

#[tauri::command]
pub async fn list_orchestra_workflows(state: State<'_, AppState>) -> Result<Vec<WorkflowListItem>, String> {
    use owl_protocol::orchestra::FailurePolicy;
    let mut items: Vec<WorkflowListItem> = state.orchestra.list_workflows().into_iter()
        .map(|w| WorkflowListItem {
            id:          w.id.0.clone(),
            name:        w.name.clone(),
            description: w.description.clone(),
            trigger:     w.trigger.clone(),
            steps: w.steps.iter().map(|s| WorkflowStepItem {
                id:      s.id.0.clone(),
                agent:   s.agent.0.clone(),
                depends: s.depends.iter().map(|d| d.0.clone()).collect(),
            }).collect(),
            on_failure: match w.on_failure {
                FailurePolicy::Abort      => "abort",
                FailurePolicy::RetryOnce  => "retry_once",
                FailurePolicy::Continue   => "continue",
            }.to_string(),
            timeout_ms:  w.timeout_ms,
        })
        .collect();
    items.sort_by(|x, y| x.id.cmp(&y.id));
    Ok(items)
}

#[tauri::command]
pub async fn list_orchestra_commands(state: State<'_, AppState>) -> Result<Vec<CommandListItem>, String> {
    use owl_protocol::orchestra::CommandExpansion;
    let mut items: Vec<CommandListItem> = state.orchestra.list_commands().into_iter()
        .map(|c| CommandListItem {
            id:          c.id.0.clone(),
            name:        c.name.clone(),
            description: c.description.clone(),
            kind: match &c.expansion {
                CommandExpansion::Text { .. }     => "text",
                CommandExpansion::Workflow { .. } => "workflow",
                CommandExpansion::Tool { .. }     => "tool",
            },
            workflow_id: match &c.expansion {
                CommandExpansion::Workflow { workflow, .. } => Some(workflow.0.clone()),
                _ => None,
            },
        })
        .collect();
    items.sort_by(|x, y| x.id.cmp(&y.id));
    Ok(items)
}
