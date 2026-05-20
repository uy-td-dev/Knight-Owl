//! System-prompt composition.
//!
//! An agent's effective prompt = base `system.md` body **plus** the body of
//! the most relevant active skills.
//!
//! ## Skill selection strategy
//!
//! When a `task_hint` is available (e.g. the workflow step's rendered prompt),
//! skills are **ranked by keyword overlap** with the hint and only the top-N
//! (default 5) are composed into the system prompt.  This avoids dumping all
//! 50 skills (25K+ tokens) into every LLM call when only 2-3 are relevant.
//!
//! Without a hint, all declared skills are included (back-compat with the
//! single-agent flow where the agent spec is hand-curated).
//!
//! Output format (stable — used by harness baselines):
//!
//! ```text
//! <agent.system_prompt>
//!
//! ---
//!
//! ## Active skills
//!
//! ### <skill_1.name>
//! _<skill_1.description>_
//!
//! <skill_1.body>
//! ```
//!
//! When the agent has zero skills the function returns the system prompt
//! unchanged.

use std::sync::Arc;

use owl_protocol::orchestra::{AgentSpec, SkillSpec};

use crate::registry::Registry;

const MAX_COMPOSED_SKILLS: usize = 5;

/// Build the effective system prompt from a base agent + an explicit skill list.
///
/// Use this when the caller already has resolved skills (e.g. in tests, or
/// after a registry lookup). For the registry-driven path, prefer
/// [`compose_for_agent`] which handles missing-skill warnings.
pub fn compose_system_prompt(agent: &AgentSpec, skills: &[Arc<SkillSpec>]) -> String {
    if skills.is_empty() {
        return agent.system_prompt.trim_end().to_string();
    }
    let mut out = String::with_capacity(agent.system_prompt.len() + 1024);
    out.push_str(agent.system_prompt.trim_end());
    out.push_str("\n\n---\n\n## Active skills\n");
    for s in skills {
        out.push_str(&format!(
            "\n### {name}\n_{desc}_\n\n{body}\n",
            name = s.name,
            desc = s.description,
            body = s.body.trim(),
        ));
    }
    out
}

/// Resolve `agent.skills` against a [`Registry`] and compose.
///
/// Skill ids that don't resolve are silently skipped — the registry already
/// emits a warning event for cross-reference issues, and the composed prompt
/// is what the agent will actually see, so dropping unknown ids keeps the
/// runtime path predictable.
pub fn compose_for_agent(agent: &AgentSpec, registry: &dyn Registry) -> (String, Vec<String>) {
    let mut skills:  Vec<Arc<SkillSpec>> = Vec::with_capacity(agent.skills.len());
    let mut missing: Vec<String>         = Vec::new();
    for sid in &agent.skills {
        match registry.skill(sid) {
            Some(s) => skills.push(s),
            None    => missing.push(sid.0.clone()),
        }
    }
    (compose_system_prompt(agent, &skills), missing)
}

/// Rank skills by keyword overlap with `task_hint` and return the top N.
///
/// Used by the workflow engine to avoid composing irrelevant skills into the
/// system prompt.  Skills whose name, description, or trigger contain words
/// from the task hint score higher.  When skills ≤ MAX_COMPOSED_SKILLS, all
/// are returned (no filtering needed).
pub fn rank_skills(
    skills: &[Arc<SkillSpec>],
    task_hint: &str,
    max: usize,
) -> Vec<Arc<SkillSpec>> {
    if skills.len() <= max {
        return skills.to_vec();
    }

    let hint_words: Vec<String> = task_hint
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_lowercase())
        .collect();

    if hint_words.is_empty() {
        return skills.iter().take(max).cloned().collect();
    }

    let mut scored: Vec<(usize, &Arc<SkillSpec>)> = skills
        .iter()
        .map(|s| {
            let haystack = format!(
                "{} {} {} {}",
                s.name,
                s.description,
                s.trigger.as_deref().unwrap_or(""),
                s.body,
            )
            .to_lowercase();
            let score = hint_words
                .iter()
                .filter(|w| haystack.contains(w.as_str()))
                .count();
            (score, s)
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(max).map(|(_, s)| s.clone()).collect()
}

/// Compose with skill relevance filtering.
///
/// When a workflow step has a known prompt, pass it as `task_hint` to filter
/// skills down to the most relevant ones.  Saves tokens on agents with many
/// declared skills.
pub fn compose_system_prompt_relevant(
    agent: &AgentSpec,
    skills: &[Arc<SkillSpec>],
    task_hint: &str,
) -> String {
    let ranked = rank_skills(skills, task_hint, MAX_COMPOSED_SKILLS);
    compose_system_prompt(agent, &ranked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use owl_protocol::orchestra::{
        AgentId, ModelRef, ModelSpec, ProviderRef, SkillId, SCHEMA_VERSION,
    };

    fn base_agent() -> AgentSpec {
        AgentSpec {
            schema_version: SCHEMA_VERSION,
            id:           AgentId::new("alice").unwrap(),
            name:         "Alice".into(),
            description:  "test".into(),
            model:        Some(ModelSpec { provider: ProviderRef::Inherit, id: ModelRef::new("x") }),
            system_prompt: "You are Alice.".into(),
            max_steps:     12,
            allowed_tools: vec![],
            skills:        vec![],
            can_spawn:     false,
            max_depth:     0,
            max_input_tokens:  None,
            max_output_tokens: None,
        }
    }

    fn skill(id: &str, name: &str, body: &str) -> Arc<SkillSpec> {
        Arc::new(SkillSpec {
            schema_version: SCHEMA_VERSION,
            id:             SkillId::new(id).unwrap(),
            name:           name.into(),
            description:    "describe".into(),
            trigger:        None,
            recommended_tools: vec![],
            body:           body.into(),
        })
    }

    #[test]
    fn no_skills_returns_unchanged() {
        let a = base_agent();
        assert_eq!(compose_system_prompt(&a, &[]), "You are Alice.");
    }

    #[test]
    fn appends_skills_in_order() {
        let a = base_agent();
        let s1 = skill("one", "First Skill",  "Body of one.");
        let s2 = skill("two", "Second Skill", "Body of two.");
        let out = compose_system_prompt(&a, &[s1, s2]);
        assert!(out.starts_with("You are Alice."));
        let pos1 = out.find("First Skill").unwrap();
        let pos2 = out.find("Second Skill").unwrap();
        assert!(pos1 < pos2);
        assert!(out.contains("Body of one."));
        assert!(out.contains("Body of two."));
    }

    #[test]
    fn missing_skills_are_reported_not_panicked() {
        use crate::registry::InMemoryRegistry;

        let mut a = base_agent();
        a.skills = vec![SkillId::new("absent").unwrap()];
        let reg = InMemoryRegistry::new();
        let (prompt, missing) = compose_for_agent(&a, &reg);
        assert_eq!(prompt, "You are Alice."); // No skills resolved → base only
        assert_eq!(missing, vec!["absent".to_string()]);
    }

    #[test]
    fn rank_skills_returns_all_when_under_cap() {
        let skills = vec![
            skill("aa", "Alpha", "alpha body"),
            skill("bb", "Beta",  "beta body"),
        ];
        let ranked = rank_skills(&skills, "anything", 5);
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn rank_skills_filters_by_relevance() {
        let skills = vec![
            skill("testing", "Testing",    "write unit tests for code"),
            skill("deploy",  "Deploy",     "deploy to production servers"),
            skill("review",  "CodeReview", "review code for security bugs"),
            skill("logging", "Logging",    "add structured logging"),
            skill("migrate", "Migration",  "database migration scripts"),
            skill("refactor","Refactoring","refactor code to improve readability"),
        ];
        let ranked = rank_skills(&skills, "write tests for the payment handler", 3);
        assert_eq!(ranked.len(), 3);
        // "testing" skill should rank highest — matches "write" and "tests"
        assert_eq!(ranked[0].id.as_str(), "testing");
    }

    #[test]
    fn rank_skills_with_empty_hint_takes_first_n() {
        let skills = vec![
            skill("aa", "A", "a"),
            skill("bb", "B", "b"),
            skill("cc", "C", "c"),
        ];
        let ranked = rank_skills(&skills, "", 2);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].id.as_str(), "aa");
        assert_eq!(ranked[1].id.as_str(), "bb");
    }
}
