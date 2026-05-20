//! Spec registry — the in-memory store of all loaded agents / skills /
//! workflows / commands.
//!
//! The trait is **read-mostly**: lookups are sync, mutations are async.
//! Mutations broadcast a [`RegistryEvent`] so file watchers (later phase)
//! and UI layers can react without polling.
//!
//! Cross-reference resolution is best-effort and incremental:
//! - Registering an agent with a missing `skill_id` succeeds, but emits a
//!   warning event — this lets users edit files in any order without
//!   tripping over chicken-and-egg ordering.  Final hard validation is
//!   the caller's responsibility just before *executing* the agent.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use owl_protocol::orchestra::{
    AgentId, AgentSpec, CommandId, CommandSpec, SkillId, SkillSpec, SpecKind, WorkflowId,
    WorkflowSpec,
};
use tokio::sync::broadcast;

use crate::error::OrchestraError;

/// Read-only API the rest of the system uses.
pub trait Registry: Send + Sync {
    fn agent(&self, id: &AgentId)       -> Option<Arc<AgentSpec>>;
    fn skill(&self, id: &SkillId)       -> Option<Arc<SkillSpec>>;
    fn workflow(&self, id: &WorkflowId) -> Option<Arc<WorkflowSpec>>;
    fn command(&self, id: &CommandId)   -> Option<Arc<CommandSpec>>;

    fn list_agents(&self)    -> Vec<Arc<AgentSpec>>;
    fn list_skills(&self)    -> Vec<Arc<SkillSpec>>;
    fn list_workflows(&self) -> Vec<Arc<WorkflowSpec>>;
    fn list_commands(&self)  -> Vec<Arc<CommandSpec>>;

    /// Return a deep snapshot for harness / replay use.
    fn snapshot(&self) -> RegistrySnapshot;

    /// Subscribe to mutation events.  Returned receiver may lag if a slow
    /// consumer falls behind — see `tokio::sync::broadcast` semantics.
    fn subscribe(&self) -> broadcast::Receiver<RegistryEvent>;
}

/// Mutation events — emitted on every register / replace / unregister.
#[derive(Debug, Clone)]
pub enum RegistryEvent {
    Registered   { kind: SpecKind, id: String },
    Replaced     { kind: SpecKind, id: String },
    Unregistered { kind: SpecKind, id: String },
    /// Non-fatal validation issue (e.g. unresolved skill reference).
    Warning      { kind: SpecKind, id: String, message: String },
}

/// Deep, owned snapshot of every spec currently in the registry.
///
/// Used by the harness to fixture a known-good world before replaying a run.
#[derive(Debug, Default, Clone)]
pub struct RegistrySnapshot {
    pub agents:    Vec<Arc<AgentSpec>>,
    pub skills:    Vec<Arc<SkillSpec>>,
    pub workflows: Vec<Arc<WorkflowSpec>>,
    pub commands:  Vec<Arc<CommandSpec>>,
}

// ─── In-memory implementation ────────────────────────────────────────────────

/// Default registry implementation.  Backed by an `RwLock<Inner>` for
/// cheap concurrent reads and atomic mutations.
pub struct InMemoryRegistry {
    inner:    RwLock<Inner>,
    events:   broadcast::Sender<RegistryEvent>,
}

#[derive(Default)]
struct Inner {
    agents:    HashMap<AgentId,    Arc<AgentSpec>>,
    skills:    HashMap<SkillId,    Arc<SkillSpec>>,
    workflows: HashMap<WorkflowId, Arc<WorkflowSpec>>,
    commands:  HashMap<CommandId,  Arc<CommandSpec>>,
    /// Source path → last successful registration (for [`OrchestraError::DuplicateId`] reporting).
    sources:   HashMap<(SpecKind, String), PathBuf>,
}

impl InMemoryRegistry {
    pub fn new() -> Self {
        // Capacity 64 is plenty — registry events are infrequent (file edits,
        // not every keystroke).  Falling behind = stale UI, not a bug.
        let (events, _) = broadcast::channel(64);
        Self { inner: RwLock::new(Inner::default()), events }
    }

    // ── Mutations ────────────────────────────────────────────────────────────

    pub fn register_agent(&self, spec: AgentSpec, source: PathBuf) -> Result<(), OrchestraError> {
        self.register::<AgentId, AgentSpec>(SpecKind::Agent, spec.id.clone(), spec, source, |i| &mut i.agents)
    }
    pub fn register_skill(&self, spec: SkillSpec, source: PathBuf) -> Result<(), OrchestraError> {
        self.register::<SkillId, SkillSpec>(SpecKind::Skill, spec.id.clone(), spec, source, |i| &mut i.skills)
    }
    pub fn register_workflow(&self, spec: WorkflowSpec, source: PathBuf) -> Result<(), OrchestraError> {
        self.register::<WorkflowId, WorkflowSpec>(SpecKind::Workflow, spec.id.clone(), spec, source, |i| &mut i.workflows)
    }
    pub fn register_command(&self, spec: CommandSpec, source: PathBuf) -> Result<(), OrchestraError> {
        self.register::<CommandId, CommandSpec>(SpecKind::Command, spec.id.clone(), spec, source, |i| &mut i.commands)
    }

    /// Generic register helper — atomic check-and-insert with duplicate
    /// detection across roots (workspace + global).
    fn register<I, S>(
        &self,
        kind:    SpecKind,
        id:      I,
        spec:    S,
        source:  PathBuf,
        select:  impl FnOnce(&mut Inner) -> &mut HashMap<I, Arc<S>>,
    ) -> Result<(), OrchestraError>
    where
        I: std::hash::Hash + Eq + Clone + AsRef<str>,
    {
        let id_string = id.as_ref().to_string();
        let key = (kind, id_string.clone());

        let mut inner = self.inner.write().expect("registry poisoned");

        // Duplicate-id detection: same id from a different source is rejected.
        if let Some(prev_source) = inner.sources.get(&key) {
            if prev_source != &source {
                return Err(OrchestraError::DuplicateId {
                    kind,
                    id:        id_string,
                    existing:  prev_source.clone(),
                    incoming:  source,
                });
            }
        }

        let map = select(&mut inner);
        let was_present = map.contains_key(&id);
        map.insert(id, Arc::new(spec));
        inner.sources.insert(key, source);
        drop(inner);

        let evt = if was_present {
            RegistryEvent::Replaced { kind, id: id_string }
        } else {
            RegistryEvent::Registered { kind, id: id_string }
        };
        let _ = self.events.send(evt); // ignore "no subscribers"
        Ok(())
    }

    /// Remove a spec by kind + id.  No-op if not present.
    pub fn unregister(&self, kind: SpecKind, id: &str) {
        let mut inner = self.inner.write().expect("registry poisoned");
        let removed = match kind {
            SpecKind::Agent    => inner.agents.remove(&AgentId(id.to_string())).is_some(),
            SpecKind::Skill    => inner.skills.remove(&SkillId(id.to_string())).is_some(),
            SpecKind::Workflow => inner.workflows.remove(&WorkflowId(id.to_string())).is_some(),
            SpecKind::Command  => inner.commands.remove(&CommandId(id.to_string())).is_some(),
        };
        inner.sources.remove(&(kind, id.to_string()));
        drop(inner);
        if removed {
            let _ = self.events.send(RegistryEvent::Unregistered { kind, id: id.to_string() });
        }
    }
}

impl Default for InMemoryRegistry {
    fn default() -> Self { Self::new() }
}

impl Registry for InMemoryRegistry {
    fn agent(&self, id: &AgentId) -> Option<Arc<AgentSpec>> {
        self.inner.read().ok()?.agents.get(id).cloned()
    }
    fn skill(&self, id: &SkillId) -> Option<Arc<SkillSpec>> {
        self.inner.read().ok()?.skills.get(id).cloned()
    }
    fn workflow(&self, id: &WorkflowId) -> Option<Arc<WorkflowSpec>> {
        self.inner.read().ok()?.workflows.get(id).cloned()
    }
    fn command(&self, id: &CommandId) -> Option<Arc<CommandSpec>> {
        self.inner.read().ok()?.commands.get(id).cloned()
    }

    fn list_agents(&self) -> Vec<Arc<AgentSpec>> {
        self.inner.read().map(|i| i.agents.values().cloned().collect()).unwrap_or_default()
    }
    fn list_skills(&self) -> Vec<Arc<SkillSpec>> {
        self.inner.read().map(|i| i.skills.values().cloned().collect()).unwrap_or_default()
    }
    fn list_workflows(&self) -> Vec<Arc<WorkflowSpec>> {
        self.inner.read().map(|i| i.workflows.values().cloned().collect()).unwrap_or_default()
    }
    fn list_commands(&self) -> Vec<Arc<CommandSpec>> {
        self.inner.read().map(|i| i.commands.values().cloned().collect()).unwrap_or_default()
    }

    fn snapshot(&self) -> RegistrySnapshot {
        let inner = match self.inner.read() {
            Ok(i) => i,
            Err(_) => return RegistrySnapshot::default(),
        };
        RegistrySnapshot {
            agents:    inner.agents.values().cloned().collect(),
            skills:    inner.skills.values().cloned().collect(),
            workflows: inner.workflows.values().cloned().collect(),
            commands:  inner.commands.values().cloned().collect(),
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<RegistryEvent> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use owl_protocol::orchestra::{ModelRef, ModelSpec, ProviderRef, ToolName, SCHEMA_VERSION};

    fn agent_fixture(id: &str) -> AgentSpec {
        AgentSpec {
            schema_version: SCHEMA_VERSION,
            id:           AgentId::new(id).unwrap(),
            name:         id.into(),
            description:  "test".into(),
            model:        Some(ModelSpec {
                provider: ProviderRef::Inherit,
                id:       ModelRef::new("x"),
            }),
            system_prompt: "You are.".into(),
            max_steps:     12,
            allowed_tools: vec![ToolName::new("read_file")],
            skills:        vec![],
            can_spawn:     false,
            max_depth:     0,
            max_input_tokens:  None,
            max_output_tokens: None,
        }
    }

    #[test]
    fn register_then_lookup() {
        let r = InMemoryRegistry::new();
        r.register_agent(agent_fixture("alice"), PathBuf::from("/a"))
            .expect("first register");
        let got = r.agent(&AgentId::new("alice").unwrap()).expect("found");
        assert_eq!(got.id.as_str(), "alice");
        assert_eq!(r.list_agents().len(), 1);
    }

    #[test]
    fn replace_emits_replaced_event() {
        let r = InMemoryRegistry::new();
        let mut rx = r.subscribe();

        r.register_agent(agent_fixture("alice"), PathBuf::from("/a"))
            .expect("first register");
        // Same source path → counts as replace, not duplicate.
        r.register_agent(agent_fixture("alice"), PathBuf::from("/a"))
            .expect("replace");

        let e1 = rx.try_recv().expect("first event");
        let e2 = rx.try_recv().expect("second event");
        assert!(matches!(e1, RegistryEvent::Registered { .. }));
        assert!(matches!(e2, RegistryEvent::Replaced   { .. }));
    }

    #[test]
    fn duplicate_from_different_source_rejected() {
        let r = InMemoryRegistry::new();
        r.register_agent(agent_fixture("alice"), PathBuf::from("/workspace/.knight-owl/agents/alice"))
            .expect("first");
        let err = r.register_agent(agent_fixture("alice"), PathBuf::from("/home/u/.knight-owl/agents/alice"))
            .expect_err("must reject duplicate");
        assert!(matches!(err, OrchestraError::DuplicateId { .. }));
    }

    #[test]
    fn unregister_removes_and_emits() {
        let r = InMemoryRegistry::new();
        let mut rx = r.subscribe();
        r.register_agent(agent_fixture("alice"), PathBuf::from("/a")).unwrap();
        r.unregister(SpecKind::Agent, "alice");
        let _ = rx.try_recv();
        let e2 = rx.try_recv().expect("expected unregister event");
        assert!(matches!(e2, RegistryEvent::Unregistered { .. }));
        assert!(r.agent(&AgentId::new("alice").unwrap()).is_none());
    }

    #[test]
    fn snapshot_captures_all_kinds() {
        let r = InMemoryRegistry::new();
        r.register_agent(agent_fixture("alice"), PathBuf::from("/a")).unwrap();
        r.register_agent(agent_fixture("bob"),   PathBuf::from("/b")).unwrap();
        let snap = r.snapshot();
        assert_eq!(snap.agents.len(), 2);
    }
}
