//! `owl chat` subcommand.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use clap::Args;
use tracing::{info, warn};

use owl_armory::registry::build_registry_with_root;
use owl_armory::traits::NativeTool;
use owl_brain::memory::InMemoryStore;
use owl_brain::reasoning_loop::ToolExecutor;
use owl_brain::{BrainConfig, BrainError, ReasoningConfig, ReasoningLoop};
use owl_protocol::sandbox::{ExecutionPlan, Sandbox};
use owl_protocol::tools::{ToolCall, ToolResult};
use owl_sandbox::local::LocalSandbox;
use owl_tower::adapters::claude::build_claude_model;
use owl_tower::config::{Config as TowerConfig, Provider};
#[cfg(feature = "gemini")]
use owl_tower::adapters::gemini::{build_gemini_model, GeminiConfig};
use owl_vault::{HashEmbedder, SurrealExperienceStore, VaultConfig};

/// Send a message to the agent and print the response.
#[derive(Args)]
pub struct ChatCommand {
    /// The message to send.
    pub message: String,

    /// Output raw JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,

    /// Workspace root for code context, file tools, and sandbox verification.
    ///
    /// Enables: graph context, read_file / write_file / run_command, persistent
    /// memory, and post-task `cargo check` verification (R-21).
    /// Run `owl index <workspace>` first to populate the code graph.
    #[arg(long, value_name = "PATH")]
    pub workspace: Option<PathBuf>,

    /// Resume a previous session by id.
    ///
    /// When omitted a new UUID session is generated and printed with `--json`.
    #[arg(long, value_name = "UUID")]
    pub session: Option<String>,

    /// LLM provider to use: `claude` (default) or `gemini`.
    ///
    /// Overrides `OWL_TOWER_PROVIDER`. Gemini requires `GEMINI_API_KEY` env var.
    #[arg(long, value_name = "PROVIDER")]
    pub provider: Option<String>,

    /// Skip sandbox verification after task completion.
    ///
    /// ⚠ Violates R-21. Use only for read-only queries that make no edits.
    #[arg(long, default_value_t = false)]
    pub no_sandbox: bool,
}

impl ChatCommand {
    pub async fn run(self) -> Result<()> {
        let brain_cfg = BrainConfig::load();
        let mut tower_cfg = TowerConfig::load();

        // CLI --provider flag overrides config/env.
        if let Some(ref p) = self.provider {
            tower_cfg.provider = match p.to_lowercase().as_str() {
                "gemini" => Provider::Gemini,
                "ollama" => Provider::Ollama,
                _        => Provider::Anthropic,
            };
        }

        // Auto-classify model size → picks the right prompt + budgets so
        // tiny local models don't drown in the full agent surface.
        let mut rl_cfg = ReasoningConfig::for_model_id(&tower_cfg.model);
        rl_cfg.max_steps = brain_cfg.max_steps;
        if brain_cfg.memory_context_limit > 0 {
            rl_cfg.memory_context_limit =
                brain_cfg.memory_context_limit.min(rl_cfg.memory_context_limit);
        }

        let session_id = self.session.clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let workspace_root = self.workspace.clone()
            .map(|p| p.canonicalize().unwrap_or(p))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        // Memory: SurrealDB when workspace given, in-process otherwise.
        let memory: Arc<dyn owl_brain::MemoryStore> = if self.workspace.is_some() {
            match build_surreal_memory(&session_id).await {
                Some(s) => {
                    if self.session.is_none() {
                        info!(session_id, "new persistent session");
                    } else {
                        info!(session_id, "resuming session");
                    }
                    Arc::new(s)
                }
                None => Arc::new(InMemoryStore::new()),
            }
        } else {
            Arc::new(InMemoryStore::new())
        };

        // Tools.
        let mut tool_vec: Vec<Box<dyn NativeTool>> =
            build_registry_with_root(workspace_root.clone());

        // Graph context + SearchCodeTool + ExperienceStore (when SurrealDB reachable).
        let store = if self.workspace.is_some() { build_store().await } else { None };

        let ctx_provider = store.as_ref().map(|s| {
            let p = crate::context::GraphContextProvider::new(Arc::clone(s));
            Arc::new(p) as Arc<dyn owl_brain::ContextProvider>
        });

        let experience: Option<Arc<dyn owl_brain::ExperienceStore>> =
            store.as_ref().map(|s| {
                Arc::new(SurrealExperienceStore::new(Arc::clone(s)))
                    as Arc<dyn owl_brain::ExperienceStore>
            });

        if let Some(ref s) = store {
            tool_vec.push(Box::new(crate::tools::search_code::SearchCodeTool {
                store: Arc::clone(s),
            }));
        }

        let tools: Arc<dyn owl_brain::reasoning_loop::ToolExecutor> =
            Arc::new(ArmoryExecutor { tools: tool_vec });

        macro_rules! build_and_run {
            ($model:expr) => {{
                let mut loop_ = ReasoningLoop::new($model, Arc::clone(&tools), Arc::clone(&memory), rl_cfg.clone());
                if let Some(ctx) = ctx_provider {
                    loop_ = loop_.with_context(ctx);
                }
                if let Some(exp) = experience {
                    loop_ = loop_.with_experience(exp);
                }
                loop_.run(&self.message).await?
            }};
        }

        let response = match tower_cfg.provider {
            #[cfg(feature = "gemini")]
            Provider::Gemini => {
                let gcfg = GeminiConfig {
                    model:           tower_cfg.model.clone(),
                    embedding_model: tower_cfg.embedding_model.clone(),
                    api_key:         tower_cfg.gemini_api_key.clone(),
                };
                build_and_run!(build_gemini_model(gcfg)?)
            }
            _ => build_and_run!(build_claude_model(tower_cfg)?),
        };

        // ── R-21 Sandbox verification ──────────────────────────────────────
        // After any task that may have written files, verify with cargo check.
        // Skip when --no-sandbox is set (read-only queries).
        if self.workspace.is_some() && !self.no_sandbox {
            let task_id = uuid::Uuid::new_v4().to_string();
            let plan = ExecutionPlan::cargo_check(
                workspace_root.to_string_lossy(),
                &task_id,
            );
            let sandbox: Arc<dyn Sandbox> = Arc::new(LocalSandbox::new());
            match sandbox.run(plan).await {
                Ok(outcome) => {
                    if outcome.success {
                        info!(duration_ms = outcome.duration_ms, "sandbox: cargo check passed");
                    } else {
                        warn!(
                            exit_code = outcome.exit_code,
                            stderr = %outcome.stderr,
                            "sandbox: cargo check FAILED — review edits before committing"
                        );
                    }
                }
                Err(e) => warn!(err = %e, "sandbox run failed (non-fatal)"),
            }
        }

        if self.json {
            println!(
                "{}",
                serde_json::json!({ "response": response, "session_id": session_id })
            );
        } else {
            println!("{response}");
        }
        Ok(())
    }
}

/// Connect to SurrealDB using `VaultConfig::load()`.
async fn build_store() -> Option<Arc<dyn owl_vault::HybridStore>> {
    let cfg = VaultConfig::load();
    match owl_vault::surreal::SurrealStore::connect(cfg.into_surreal()).await {
        Ok(store) => {
            info!("SurrealDB connected");
            Some(Arc::new(store) as Arc<dyn owl_vault::HybridStore>)
        }
        Err(e) => {
            warn!(err = %e, "SurrealDB unreachable — running without graph context");
            None
        }
    }
}

/// Connect a `SurrealMemoryStore` with `HashEmbedder` for Level 2 semantic recall.
async fn build_surreal_memory(
    session_id: &str,
) -> Option<owl_vault::SurrealMemoryStore> {
    let cfg = VaultConfig::load();
    let embedder: Arc<dyn owl_vault::Embedder> = Arc::new(HashEmbedder::default());
    owl_vault::SurrealMemoryStore::connect(
        cfg.into_surreal(),
        session_id.to_string(),
        Some(embedder),
    )
    .await
    .ok()
}

/// Bridges all registered tools to the brain `ToolExecutor` trait.
struct ArmoryExecutor {
    tools: Vec<Box<dyn NativeTool>>,
}

#[async_trait]
impl ToolExecutor for ArmoryExecutor {
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, BrainError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == call.name)
            .ok_or_else(|| BrainError::ToolDispatch(format!("unknown tool: {}", call.name)))?;
        tool.run(call).await.map_err(|e| BrainError::ToolDispatch(e.to_string()))
    }

    fn has_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t.name() == name)
    }
}
