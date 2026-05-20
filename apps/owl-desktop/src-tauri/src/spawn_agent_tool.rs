//! `spawn_agent` — native tool that lets a parent agent dispatch a sub-agent
//! task at runtime.
//!
//! v1 scope (Design Spec §0 A4 + Phase 9 plan):
//!
//! - Sub-agents run **synchronously**; parent blocks on the child's final
//!   answer.  Streaming nested events back to the UI is deferred to a
//!   follow-up phase — for now the child's full text appears as the tool
//!   result, which the parent can quote / summarise.
//! - **Single-level only**: parent depth is hardcoded 0, child depth 1.
//!   The child's spec must have `max_depth >= 1`; if it tries to call
//!   `spawn_agent` itself the [`AgentFactory::build`] depth check refuses.
//! - `profile` (an existing agent id from the orchestra registry) is the
//!   recommended path — composes that agent's persona, skills, and tool
//!   allowlist.  Ad-hoc spawning without a profile is intentionally NOT
//!   supported in v1: keeps the trust boundary tight (only registered
//!   agents can run) and avoids users having to redefine system prompts
//!   inline.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use owl_armory::traits::NativeTool;
use owl_armory::ArmoryError;
use owl_brain::AgentFactory;
use owl_orchestra::{Registry, compose::compose_for_agent};
use owl_protocol::orchestra::AgentId;
use owl_protocol::tools::{ToolCall, ToolResult};

/// Hard cap on time spent inside `spawn_agent.run` — guards against a child
/// agent's runaway tool loop blocking the parent indefinitely.
const SUBAGENT_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum length of the sub-agent answer returned to the parent — anything
/// longer is truncated with a note.  Keeps token usage on the parent side
/// bounded.
const MAX_RESULT_CHARS: usize = 8_000;

/// `spawn_agent` arguments.
#[derive(Debug, Deserialize)]
struct SpawnArgs {
    /// One-line description of the sub-task — shown in the parent's tool chip.
    description: String,
    /// The actual instruction handed to the child agent.
    prompt: String,
    /// Agent id from `<workspace>/.knight-owl/agents/`.  Required in v1.
    profile: String,
}

/// Late-bound factory handle.
///
/// Bootstrap order forces a chicken-and-egg: the agent factory captures the
/// session's tool executor, and `SpawnAgentTool` lives inside that same
/// executor.  We solve it by giving the tool an `Arc<RwLock<Option<...>>>`
/// that the host populates immediately after the factory is built.  Any
/// `run()` call before binding returns a clear error.
pub type FactoryHandle = Arc<tokio::sync::RwLock<Option<Arc<dyn AgentFactory>>>>;

pub struct SpawnAgentTool {
    factory:  FactoryHandle,
    registry: Arc<dyn Registry>,
    /// Depth of the immediate caller — fixed at 0 for the chat root in v1.
    /// Sub-agents thread this via call-context in a future phase.
    parent_depth: u8,
}

impl SpawnAgentTool {
    pub fn new(factory: FactoryHandle, registry: Arc<dyn Registry>) -> Self {
        Self { factory, registry, parent_depth: 0 }
    }

    /// Convenience: build a fresh handle for the host to pass into both the
    /// tool (so it can read) and `set_factory` (so it can write).
    pub fn new_handle() -> FactoryHandle {
        Arc::new(tokio::sync::RwLock::new(None))
    }
}

#[async_trait]
impl NativeTool for SpawnAgentTool {
    fn name(&self) -> &'static str { "spawn_agent" }

    fn description(&self) -> &'static str {
        "Spawn a focused sub-agent to handle a self-contained sub-task.  \
         Pick `profile` from the workspace's registered agents (e.g. \
         `researcher`, `reviewer`).  The sub-agent runs to completion and \
         returns its final text answer here — quote / summarise it in your \
         next reply.  Use this for parallel research, narrow audits, or \
         any task that doesn't need full chat context."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: SpawnArgs = serde_json::from_value(call.args)
            .map_err(|e| ArmoryError::InvalidArgs(format!("spawn_agent: {e}")))?;

        // Resolve the requested profile.
        let agent_id = AgentId::new_reserved(&args.profile)
            .map_err(|e| ArmoryError::InvalidArgs(format!("invalid profile id: {e}")))?;
        let spec = self.registry.agent(&agent_id)
            .ok_or_else(|| ArmoryError::Execution(
                format!("unknown agent profile `{}`. \
                         Drop an `agent.toml` under .knight-owl/agents/ to register one.",
                        args.profile)
            ))?;

        // Compose system prompt with that agent's skills.
        let mut skills = Vec::with_capacity(spec.skills.len());
        for sid in &spec.skills {
            if let Some(s) = self.registry.skill(sid) {
                skills.push(s);
            }
        }
        let _ = compose_for_agent; // (compose happens inside factory.build)

        // Build the child runner.  `parent_depth + 1` is the child's depth;
        // the factory enforces `depth <= spec.max_depth` so a child whose
        // spec has `max_depth = 0` will be refused here — surface the error
        // back to the LLM as the tool result so it can recover.
        let depth = self.parent_depth + 1;
        let factory_lock = self.factory.read().await;
        let factory = factory_lock.as_ref().ok_or_else(|| ArmoryError::Execution(
            "spawn_agent: factory not bound yet (host startup race)".into()
        ))?;
        let runner = factory.build(spec.clone(), skills, depth).await
            .map_err(|e| ArmoryError::Execution(format!("spawn refused: {e}")))?;
        drop(factory_lock);

        // Run the child to completion under a hard timeout.
        let prompt = args.prompt.clone();
        let answer = match tokio::time::timeout(SUBAGENT_TIMEOUT, runner.run(&prompt)).await {
            Ok(Ok(a))  => a,
            Ok(Err(e)) => return Err(ArmoryError::Execution(format!("sub-agent error: {e}"))),
            Err(_)     => return Err(ArmoryError::Execution(format!(
                "sub-agent timed out after {}s", SUBAGENT_TIMEOUT.as_secs()
            ))),
        };

        // Truncate if huge.
        let mut output = answer;
        let trunc = output.len() > MAX_RESULT_CHARS;
        if trunc {
            output.truncate(MAX_RESULT_CHARS);
            output.push_str("\n…(truncated)");
        }

        // Pack a structured result so the UI's workflow-step renderer also
        // recognises sub-agent results — same `status`/`agent`/`output`
        // shape as `WorkflowStepResult` (see App.tsx parseWorkflowStep).
        let payload = serde_json::json!({
            "status":      "completed",
            "agent":       args.profile,
            "description": args.description,
            "depth":       depth,
            "truncated":   trunc,
            "output":      output,
        });
        Ok(ToolResult::ok(self.name(), payload))
    }
}
