//! Concrete [`owl_brain::AgentFactory`] for the desktop host.
//!
//! Builds [`owl_brain::AgentRunner`] instances from an `AgentSpec` + resolved
//! skills.  The factory holds the session-default LLM model + the shared
//! tool executor + memory store; per-agent fields (system prompt, tool
//! allowlist, recursion limits) are injected at `build()` time:
//!
//! - **System prompt** is composed via
//!   [`owl_orchestra::compose::compose_system_prompt`] (base + every active
//!   skill body).
//! - **Tool allowlist** wraps the shared executor in
//!   [`owl_brain::FilteredExecutor`].  An empty list = "all tools allowed"
//!   (back-compat with the pre-orchestra single-agent flow).
//! - **Recursion** is bounded: `build()` refuses when `depth > spec.max_depth`.
//!
//! ## Provider routing — v1 limitation
//!
//! `AgentSpec.model` may declare a different provider per agent
//! (e.g. one agent on Gemini, another on Anthropic).  v1 only supports the
//! **session-default provider** chosen at startup; mismatched
//! `ProviderRef::Anthropic / Ollama` log a warning and fall back to the
//! default.  Cross-provider routing requires either type-erased model
//! storage or an enum dispatch — both are deferred.

use std::sync::Arc;

use async_trait::async_trait;
use rig::completion::CompletionModel;

use owl_brain::{
    AgentFactory, AgentRunner, BrainError, ContextProvider, FilteredExecutor, MemoryStore,
    ReasoningConfig, ReasoningLoop,
    reasoning_loop::ToolExecutor,
};
use owl_orchestra::compose::compose_system_prompt;
use owl_protocol::orchestra::{AgentSpec, ProviderRef, SkillSpec};

/// Desktop-side factory.
///
/// Generic over the rig completion model `M` so we don't need a heterogenous
/// provider pool — the session ships with one provider, every agent uses it.
pub struct DesktopAgentFactory<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    /// The session-default completion model.  Cloned cheaply per spawn.
    model:   M,
    /// Shared executor — wrapped per-agent with [`FilteredExecutor`] when
    /// the spec declares a non-empty allowlist.
    executor: Arc<dyn ToolExecutor>,
    /// Shared memory store — every agent in this session shares it for now.
    memory:  Arc<dyn MemoryStore>,
    /// Optional context provider (graph-RAG seed) injected into each runner.
    ctx:     Option<Arc<dyn ContextProvider>>,
    /// Default `memory_context_limit` from the host's [`owl_brain::BrainConfig`].
    /// `max_steps` comes from the agent spec.
    memory_context_limit: usize,
    /// Provider this factory was built with — used for diagnostics when an
    /// agent requests a different one.
    provider_label: &'static str,
    /// Model size class — drives prompt + budget selection for every
    /// agent this factory spawns (so sub-agents using a 2 B local model
    /// also get the short prompt + tight budgets).
    model_class: owl_brain::ModelClass,
}

impl<M> DesktopAgentFactory<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    pub fn new(
        model:    M,
        executor: Arc<dyn ToolExecutor>,
        memory:   Arc<dyn MemoryStore>,
        ctx:      Option<Arc<dyn ContextProvider>>,
        memory_context_limit: usize,
        provider_label: &'static str,
        model_class: owl_brain::ModelClass,
    ) -> Self {
        Self { model, executor, memory, ctx, memory_context_limit, provider_label, model_class }
    }

    /// Compute the effective `ToolExecutor` for an agent — wraps the shared
    /// one with [`FilteredExecutor`] when the spec restricts tools, else
    /// reuses the shared executor as-is (zero alloc for the common path).
    fn executor_for(&self, spec: &AgentSpec) -> Arc<dyn ToolExecutor> {
        if spec.allowed_tools.is_empty() {
            Arc::clone(&self.executor)
        } else {
            let names = spec.allowed_tools.iter().map(|t| t.0.clone());
            Arc::new(FilteredExecutor::new(Arc::clone(&self.executor), names))
        }
    }

    /// Diagnose mismatched-provider requests.  Returns `true` to proceed,
    /// `false` to refuse — current policy is "warn + proceed with default"
    /// so the agent still runs (degraded is better than dead).
    fn check_provider(&self, spec: &AgentSpec) -> bool {
        if let Some(m) = &spec.model {
            if !matches!(m.provider, ProviderRef::Inherit) {
                let want = match m.provider {
                    ProviderRef::Gemini    => "gemini",
                    ProviderRef::Anthropic => "anthropic",
                    ProviderRef::Ollama    => "ollama",
                    ProviderRef::Inherit   => return true,
                };
                if want != self.provider_label {
                    tracing::warn!(
                        agent = %spec.id,
                        requested = want,
                        active    = self.provider_label,
                        "agent requests a different provider; falling back to session default",
                    );
                }
            }
        }
        true
    }
}

#[async_trait]
impl<M> AgentFactory for DesktopAgentFactory<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    async fn build(
        &self,
        spec:   Arc<AgentSpec>,
        skills: Vec<Arc<SkillSpec>>,
        depth:  u8,
    ) -> Result<Arc<dyn AgentRunner>, BrainError> {
        // Recursion guard — sub-agents must respect parent's max_depth.
        if depth > spec.max_depth {
            return Err(BrainError::Config(format!(
                "spawn refused: depth {} exceeds agent `{}` max_depth {}",
                depth, spec.id, spec.max_depth
            )));
        }
        // Provider routing diagnostic (no hard failure today).
        let _ = self.check_provider(&spec);

        // Compose system prompt: base + skill bodies (eager strategy).
        let system_prompt = compose_system_prompt(&spec, &skills);

        // Reasoning config — agent owns max_steps; memory limit + tool
        // budget come from the model-class auto-classification.  We keep
        // the agent's composed prompt (skills already merged) by
        // overriding `system_prompt` AFTER the for-model defaults pick
        // a class-appropriate prompt.
        let mut cfg = ReasoningConfig::for_model(self.model_class);
        cfg.max_steps            = spec.max_steps;
        cfg.memory_context_limit = self.memory_context_limit
            .min(cfg.memory_context_limit);
        cfg.system_prompt        = system_prompt;

        // Tool surface for this agent.
        let executor = self.executor_for(&spec);

        // Build the reasoning loop.
        let mut rl = ReasoningLoop::new(
            self.model.clone(),
            executor,
            Arc::clone(&self.memory),
            cfg,
        );
        if let Some(ctx) = &self.ctx {
            rl = rl.with_context(Arc::clone(ctx));
        }

        Ok(Arc::new(rl) as Arc<dyn AgentRunner>)
    }
}
