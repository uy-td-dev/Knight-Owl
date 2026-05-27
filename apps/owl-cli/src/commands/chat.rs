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
use owl_protocol::sandbox::Sandbox;
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

    /// Enter interactive REPL mode after sending the initial message.
    ///
    /// Each line is dispatched as a new turn; `/<cmd>` runs slash
    /// commands inline.  EOF (Ctrl-D) exits.  Session id is preserved
    /// across turns for memory continuity.
    #[arg(long, default_value_t = false)]
    pub repl: bool,
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
        // R-21: workspace_path is required for in-loop sandbox verification.
        // Without a workspace we cannot mount anything; verification silently
        // skips inside the loop.
        if self.workspace.is_some() && !self.no_sandbox {
            rl_cfg.workspace_path = self
                .workspace
                .as_ref()
                .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone())
                    .to_string_lossy()
                    .into_owned());
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

        let embedder: Arc<dyn owl_vault::Embedder> = Arc::new(HashEmbedder::default());
        let ctx_provider = store.as_ref().map(|s| {
            let mut p = crate::context::GraphContextProvider::new(Arc::clone(s))
                .with_embedder(Arc::clone(&embedder));
            // Phase G — wire cross-session recall when SurrealDB is reachable.
            // We can reuse the existing `memory` Arc (SurrealMemoryStore
            // when workspace is given, InMemoryStore otherwise).
            p = p.with_session_recall(Arc::clone(&memory), session_id.clone());
            Arc::new(p) as Arc<dyn owl_brain::ContextProvider>
        });

        let experience: Option<Arc<dyn owl_brain::ExperienceStore>> =
            store.as_ref().map(|s| {
                Arc::new(SurrealExperienceStore::new(Arc::clone(s)))
                    as Arc<dyn owl_brain::ExperienceStore>
            });

        // Phase C — slash command intercept.  When the message starts
        // with `/`, route to the dispatcher and skip the LLM entirely.
        if let Some(rest) = self.message.strip_prefix('/') {
            let dispatcher = crate::slash::SlashDispatcher {
                session_id: session_id.clone(),
                model_id:   tower_cfg.model.clone(),
                memory:     Arc::clone(&memory),
                experience: experience.clone(),
            };
            let out = dispatcher.dispatch(rest).await?;
            if self.json {
                println!("{}", serde_json::json!({
                    "response":   out.message,
                    "session_id": session_id,
                    "slash":      true,
                }));
            } else {
                println!("{}", out.message);
            }
            return Ok(());
        }

        if let Some(ref s) = store {
            tool_vec.push(Box::new(crate::tools::search_code::SearchCodeTool {
                store: Arc::clone(s),
            }));
        }

        let tools: Arc<dyn owl_brain::reasoning_loop::ToolExecutor> =
            Arc::new(ArmoryExecutor { tools: tool_vec });

        // R-21: a sandbox is wired only when verification is enabled.  When
        // None, the loop skips the verification gate and returns the model's
        // text as-is — used for --no-sandbox and workspace-less queries.
        let sandbox: Option<Arc<dyn Sandbox>> =
            if self.workspace.is_some() && !self.no_sandbox {
                Some(Arc::new(LocalSandbox::new()))
            } else {
                None
            };

        // Auto-skill writer — point at `.knight-owl/skills/` under the
        // workspace.  Available even without persistent memory so demos
        // can show one-shot distillation.
        let skill_writer: Option<Arc<dyn owl_brain::SkillWriter>> =
            self.workspace.as_ref().map(|ws| {
                let path = ws.canonicalize().unwrap_or_else(|_| ws.clone());
                Arc::new(owl_orchestra::FsSkillWriter::for_workspace(&path))
                    as Arc<dyn owl_brain::SkillWriter>
            });

        macro_rules! build_and_run {
            ($model:expr, $model_id:expr) => {{
                let mid: String = $model_id;
                let model_for_loop      = $model;
                let model_for_compactor = model_for_loop.clone();
                let mut loop_ = ReasoningLoop::new(model_for_loop, Arc::clone(&tools), Arc::clone(&memory), rl_cfg.clone());
                if let Some(ctx) = ctx_provider {
                    loop_ = loop_.with_context(ctx);
                }
                if let Some(exp) = experience.clone() {
                    loop_ = loop_.with_experience(exp);
                }
                if let Some(sb) = sandbox {
                    loop_ = loop_.with_sandbox(sb);
                }
                if let Some(sw) = skill_writer {
                    loop_ = loop_.with_skill_writer(sw);
                }
                let compactor: Arc<dyn owl_brain::Compactor> =
                    Arc::new(owl_brain::LlmCompactor::new(model_for_compactor));
                loop_ = loop_.with_compactor(compactor);

                let first = loop_.run(&self.message).await?;

                // ── REPL mode (Phase C) ──────────────────────────────
                // Print first response, then read stdin line-by-line.
                // Each line is dispatched as either a slash command or
                // a new agent turn; EOF exits gracefully.
                if self.repl {
                    use tokio::io::{AsyncBufReadExt, BufReader};

                    if self.json {
                        println!("{}", serde_json::json!({
                            "response": first, "session_id": session_id,
                        }));
                    } else {
                        println!("{first}");
                    }
                    let stdin = tokio::io::stdin();
                    let mut reader = BufReader::new(stdin).lines();
                    eprint!("> ");
                    while let Some(line) = reader.next_line().await? {
                        let line = line.trim();
                        if line.is_empty() { eprint!("> "); continue; }

                        let out = if let Some(rest) = line.strip_prefix('/') {
                            let d = crate::slash::SlashDispatcher {
                                session_id: session_id.clone(),
                                model_id:   mid.clone(),
                                memory:     Arc::clone(&memory),
                                experience: experience.clone(),
                            };
                            match d.dispatch(rest).await {
                                Ok(o)  => o.message,
                                Err(e) => format!("error: {e}"),
                            }
                        } else {
                            loop_.run(line).await
                                .unwrap_or_else(|e| format!("error: {e}"))
                        };
                        if self.json {
                            println!("{}", serde_json::json!({
                                "response": out, "session_id": session_id,
                            }));
                        } else {
                            println!("{out}");
                        }
                        eprint!("> ");
                    }
                    eprintln!();
                    return Ok(());
                }

                first
            }};
        }

        // Capture model id before tower_cfg is consumed by adapter builders.
        let model_id_str = tower_cfg.model.clone();
        let response = match tower_cfg.provider {
            #[cfg(feature = "gemini")]
            Provider::Gemini => {
                let gcfg = GeminiConfig {
                    model:           tower_cfg.model.clone(),
                    embedding_model: tower_cfg.embedding_model.clone(),
                    api_key:         tower_cfg.gemini_api_key.clone(),
                };
                build_and_run!(build_gemini_model(gcfg)?, model_id_str.clone())
            }
            _ => build_and_run!(build_claude_model(tower_cfg)?, model_id_str.clone()),
        };

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
