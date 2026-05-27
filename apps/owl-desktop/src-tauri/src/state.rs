//! Shared application state injected into Tauri command handlers.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use owl_armory::registry::build_registry_with_root_and_sandbox;
use owl_brain::memory::InMemoryStore;
use owl_brain::reasoning_loop::ToolExecutor;
use owl_brain::runner::AgentRunner;
use owl_brain::{BrainConfig, BrainError, ContextProvider, ReasoningConfig, ReasoningLoop};
use owl_protocol::tools::{ToolCall, ToolResult};
use owl_tower::adapters::claude::build_claude_model;
use owl_tower::config::{Config as TowerConfig, Provider};
#[cfg(feature = "gemini")]
use owl_tower::adapters::gemini::{build_gemini_model, GeminiConfig};
#[cfg(feature = "ollama")]
use owl_tower::adapters::ollama::{build_ollama_model, OllamaConfig};
use owl_vault::{HashEmbedder, SurrealMemoryStore, VaultConfig};

/// Application-wide state available to all Tauri command handlers.
/// One turn of the conversation persisted across calls.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatTurn {
    /// `"user"` or `"model"` (Gemini role names).
    pub role: String,
    pub text: String,
}

pub struct AppState {
    pub runner:  Arc<dyn AgentRunner>,
    /// All registered tools, keyed by name for O(1) lookup.
    pub tools:   Arc<HashMap<String, Box<dyn owl_armory::NativeTool>>>,
    /// Per-conversation Gemini wire-format histories.  Each conversation has
    /// its own [`Vec<ChatTurn>`] keyed by `conv_id` so switching the active
    /// chat in the UI swaps the agent's context — old session content never
    /// leaks into a new one unless the user explicitly asks.
    ///
    /// Each `Arc<Mutex<Vec<…>>>` is independently lockable, so two
    /// conversations can run side-by-side without contending.  Lazy-loaded
    /// from disk on first lookup; persisted per-conv to `chats_dir`.
    pub histories: Arc<tokio::sync::Mutex<
        std::collections::HashMap<String, Arc<tokio::sync::Mutex<Vec<ChatTurn>>>>
    >>,
    /// Directory holding per-conversation history files
    /// (`<chats_dir>/<conv_id>.json`).  One per workspace_id.
    pub chats_dir: PathBuf,
    /// Gemini API key (kept here so commands can read it without env every call).
    pub gemini_api_key: Option<String>,
    /// Active model id (e.g. `gemini-2.5-flash`).
    pub model: String,
    /// Active workspace root — every file/grep/glob/edit tool is scoped here.
    pub workspace: PathBuf,
    /// SurrealDB store, when reachable.  Used for code-graph ingestion.
    pub store: Option<Arc<dyn owl_vault::HybridStore>>,

    /// Embedder used for code-node vector embeddings during indexing.
    pub embedder: Arc<dyn owl_vault::Embedder>,

    /// Orchestra registry — agents / skills / workflows / commands loaded
    /// from `<workspace>/.knight-owl/` (workspace) and `~/.knight-owl/` (global).
    /// Hot-reloaded by the watcher held in [`Self::orchestra_watcher`].
    pub orchestra: Arc<owl_orchestra::InMemoryRegistry>,

    /// Filesystem watcher kept alive for the lifetime of the app.
    /// Dropping it stops live-reload of the orchestra registry.
    pub orchestra_watcher: Arc<tokio::sync::Mutex<Option<owl_orchestra::OrchestraWatcher>>>,

    /// GraphRAG ingester — extracts L3 entities from text chunks.
    pub cartographer: Option<Arc<dyn owl_cartographer::GraphIngester>>,

    /// Builds [`AgentRunner`]s on demand from an [`AgentSpec`] — used by the
    /// agent picker (Phase 5) and by the `spawn_agent` tool (later phase).
    pub factory: Arc<dyn owl_brain::AgentFactory>,

    /// Active workflow runs, keyed by `trace_id`.  Holds the
    /// [`CancellationToken`] so the UI can stop a long-running workflow
    /// (Phase 8 — workflow UI polish).  Tokens are removed when the run
    /// emits a `WorkflowCompleted` / `WorkflowCancelled` event.
    pub running_workflows: Arc<tokio::sync::Mutex<
        std::collections::HashMap<String, tokio_util::sync::CancellationToken>
    >>,

    /// Pending tool-approval requests, keyed by `call_id` issued by
    /// [`crate::approval::InteractiveGate`].  The Tauri command
    /// `resolve_tool_approval` consumes the entry and forwards the user's
    /// decision via the parked `oneshot::Sender`.
    pub pending_approvals: crate::approval::PendingApprovals,

    /// Late-bound `AppHandle` slot used by the approval gate.  `setup()`
    /// populates it after the Tauri builder yields the handle; until then,
    /// the gate auto-approves to avoid deadlocks during boot.
    pub app_handle_slot: crate::approval::AppHandleSlot,

    /// Per-turn binding for the event sink: tells [`crate::event_sink::TauriEventSink`]
    /// which conversation id is currently active so events get appended to
    /// the correct `*.events.jsonl` log file.  `commands::stream` updates
    /// this on each turn before invoking the runner.
    pub active_conv: crate::event_sink::ActiveConvSlot,

    /// Owl chibi pet — gamified companion that reacts to agent events.
    /// See [`crate::pet_state::PetState`] + `commands::pet`.
    pub pet: crate::pet_state::PetState,
}

impl AppState {
    /// Initialize the reasoning loop.
    ///
    /// Reads `BrainConfig` and `TowerConfig` from `config/*.toml` + env vars.
    /// Connects to SurrealDB when `OWL_VAULT_ENDPOINT` is reachable; falls
    /// back to in-process store and no graph context if the DB is unreachable.
    pub fn new() -> Self {
        // Construct the approval-gate dependencies BEFORE the runner so they
        // can be threaded into the executor inside `build_runner`.  The
        // `app_handle_slot` is populated later, in `setup()`, once the Tauri
        // builder yields the handle.
        let pending_approvals: crate::approval::PendingApprovals = Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new()
        ));
        let app_handle_slot: crate::approval::AppHandleSlot = Arc::new(tokio::sync::RwLock::new(None));
        let active_conv: crate::event_sink::ActiveConvSlot = Arc::new(tokio::sync::RwLock::new(
            crate::event_sink::ActiveConv::default()
        ));

        let init = tauri::async_runtime::block_on(build_runner(
            Arc::clone(&pending_approvals),
            Arc::clone(&app_handle_slot),
            Arc::clone(&active_conv),
        ));

        let workspace_id = workspace_id_for(&init.workspace);
        let chats_dir    = chats_dir_for(&workspace_id);
        if let Err(e) = std::fs::create_dir_all(&chats_dir) {
            tracing::warn!(err = %e, dir = %chats_dir.display(),
                "failed to create chats dir; histories will not persist");
        }
        tracing::info!(
            dir = %chats_dir.display(),
            workspace_id = %workspace_id,
            "per-conversation chat histories enabled",
        );

        Self {
            runner:         init.runner,
            tools:          init.tools,
            histories:      Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new()
            )),
            chats_dir,
            gemini_api_key: init.gemini_api_key,
            model:          init.model,
            workspace:      init.workspace,
            store:          init.store,
            embedder:       init.embedder,
            orchestra:         init.orchestra,
            orchestra_watcher: init.orchestra_watcher,
            cartographer:      init.cartographer,
            factory:           init.factory,
            running_workflows: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new()
            )),
            pending_approvals,
            app_handle_slot,
            active_conv,
            pet: init.pet,
        }
    }

    /// Default per-tool [`ToolPolicy`] map for desktop sessions.
    ///
    /// Destructive / side-effectful tools (file writes, shell execution)
    /// are gated behind [`ToolPolicy::RequireApproval`]; all others stay
    /// [`ToolPolicy::AutoApprove`].  Hosts can override per-agent via
    /// `agent.toml`'s `allowed_tools` (handled by [`FilteredExecutor`]).
    pub fn default_tool_policies() -> std::collections::HashMap<String, owl_protocol::tools::ToolPolicy> {
        use owl_protocol::tools::ToolPolicy;
        let approve = ToolPolicy::RequireApproval;
        [
            ("bash",         approve),
            ("write_file",   approve),
            ("edit_file",    approve),
            ("multi_edit",   approve),
            ("apply_patch",  approve),
            ("run_command",  approve),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    /// Get the agent's wire-format history for `conv_id`, lazily loading it
    /// from disk if this is the first reference.  Each conv has its own
    /// `Arc<Mutex<…>>` so concurrent runs on different convs don't contend.
    pub async fn history_for(&self, conv_id: &str) -> Arc<tokio::sync::Mutex<Vec<ChatTurn>>> {
        let mut map = self.histories.lock().await;
        if let Some(h) = map.get(conv_id) { return Arc::clone(h); }

        let path = self.history_path_for(conv_id);
        let initial: Vec<ChatTurn> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<ChatTurn>>(&raw).ok())
            .unwrap_or_default();
        if !initial.is_empty() {
            tracing::info!(conv_id, turns = initial.len(), "loaded conversation history");
        }
        let h = Arc::new(tokio::sync::Mutex::new(initial));
        map.insert(conv_id.to_string(), Arc::clone(&h));
        h
    }

    /// On-disk path for a single conversation's history file.
    pub fn history_path_for(&self, conv_id: &str) -> PathBuf {
        // Sanitise the id defensively — Tauri inputs are user-controlled.
        // Allow only alphanumerics + `-_` so an exotic id can never escape
        // the chats_dir via path traversal.
        let safe: String = conv_id.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let id = if safe.is_empty() { "default".to_string() } else { safe };
        self.chats_dir.join(format!("{id}.json"))
    }
}

/// Snapshot of the bits a fresh agent needs.  Returned by [`build_runner`].
pub struct RunnerInit {
    pub runner: Arc<dyn AgentRunner>,
    pub tools:  Arc<HashMap<String, Box<dyn owl_armory::NativeTool>>>,
    pub gemini_api_key: Option<String>,
    pub model:  String,
    pub workspace: PathBuf,
    pub store: Option<Arc<dyn owl_vault::HybridStore>>,
    pub embedder: Arc<dyn owl_vault::Embedder>,
    pub orchestra: Arc<owl_orchestra::InMemoryRegistry>,
    pub orchestra_watcher: Arc<tokio::sync::Mutex<Option<owl_orchestra::OrchestraWatcher>>>,
    pub factory: Arc<dyn owl_brain::AgentFactory>,
    pub cartographer: Option<Arc<dyn owl_cartographer::GraphIngester>>,
    pub pet: crate::pet_state::PetState,
}

/// Per-workspace chats directory: `~/.knight-owl/chats/<workspace_id>/`
/// (one `*.json` file per conversation inside).
fn chats_dir_for(workspace_id: &str) -> PathBuf {
    let mut base = if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".knight-owl");
        p
    } else {
        PathBuf::from(".knight-owl")
    };
    base.push("chats");
    base.push(workspace_id);
    base
}

async fn build_runner(
    pending_approvals: crate::approval::PendingApprovals,
    app_handle_slot:   crate::approval::AppHandleSlot,
    active_conv:       crate::event_sink::ActiveConvSlot,
) -> RunnerInit {
    // Apply user-configurable settings (`~/.knight-owl/settings.json`) into
    // the process environment FIRST, before any config crate reads env vars.
    // Existing env vars always win — settings.json only fills in gaps.
    crate::commands::settings::apply_to_env(&crate::commands::settings::read_settings());

    let brain_cfg = BrainConfig::load();
    let mut tower_cfg = TowerConfig::load();

    // Auto-select provider based on which API key is available in the
    // environment, so the app works without a config file in the CWD.
    //
    // Rule: if no Anthropic key is set we default to Gemini — this prevents a
    // hard panic from `rig`'s anthropic client when neither key is present
    // (better to fail-soft with a clear error in the UI than to crash on
    // launch).  A user with both keys keeps the configured `provider` as-is.
    #[cfg(feature = "gemini")]
    if tower_cfg.api_key.is_none() && tower_cfg.gemini_api_key.is_some() {
        tower_cfg.provider = Provider::Gemini;
        if tower_cfg.model.starts_with("claude") {
            tower_cfg.model = "gemini-2.5-flash".into();
        }
    }
    // Last-resort fallback: no cloud keys at all → use local Ollama if available.
    #[cfg(feature = "ollama")]
    if tower_cfg.api_key.is_none() && tower_cfg.gemini_api_key.is_none() {
        tower_cfg.provider = Provider::Ollama;
        if tower_cfg.model.starts_with("claude") || tower_cfg.model.starts_with("gemini") {
            tower_cfg.model = "llama3.2".into();
        }
    }

    let workspace_root = workspace_root();
    let workspace_id   = workspace_id_for(&workspace_root);
    tracing::info!(
        workspace = %workspace_root.display(), id = %workspace_id,
        "active workspace",
    );
    // Per-workspace SurrealDB database — isolates indexed code, chat memory,
    // and graph relations so different workspaces never collide.  User can
    // override via env var if they want shared state across workspaces.
    if std::env::var("OWL_VAULT_DATABASE").is_err() {
        std::env::set_var("OWL_VAULT_DATABASE", format!("vault_{}", workspace_id.replace('-', "_")));
    }
    let workspace_for_state = workspace_root.clone();

    // Sandbox wire-in (R-21): when `OWL_SANDBOX_BASH=1` (or unset and default
    // policy enables it), the `bash` tool routes commands through
    // [`LocalSandbox`] instead of running them directly on the host.
    // `LocalSandbox` is a no-Docker fallback; swap to `DockerSandbox` later
    // for full container isolation.
    let bash_sandbox: Option<Arc<dyn owl_protocol::sandbox::Sandbox>> =
        match std::env::var("OWL_SANDBOX_BASH").as_deref() {
            Ok("0") | Ok("false") => None,
            _                     => Some(Arc::new(owl_sandbox::LocalSandbox::new())),
        };
    let tools_vec = build_registry_with_root_and_sandbox(workspace_root.clone(), bash_sandbox);

    // R-21 verify sandbox — runs `cargo check` after any edit-producing turn.
    // Independent of `OWL_SANDBOX_BASH`: bash isolation and verify-after-edit
    // serve different goals.  Disable with `OWL_SANDBOX_VERIFY=0`.
    let verify_sandbox: Option<Arc<dyn owl_protocol::sandbox::Sandbox>> =
        match std::env::var("OWL_SANDBOX_VERIFY").as_deref() {
            Ok("0") | Ok("false") => None,
            _                     => Some(Arc::new(owl_sandbox::LocalSandbox::new())),
        };

    // Hermes-style procedural memory: background distillation writes
    // auto-skills into <workspace>/.knight-owl/skills/.  Disable with
    // `OWL_AUTOSKILL=0`.
    let skill_writer: Option<Arc<dyn owl_brain::SkillWriter>> =
        match std::env::var("OWL_AUTOSKILL").as_deref() {
            Ok("0") | Ok("false") => None,
            _ => Some(Arc::new(
                owl_orchestra::FsSkillWriter::for_workspace(&workspace_for_state),
            ) as Arc<dyn owl_brain::SkillWriter>),
        };

    let vault_cfg = VaultConfig::load();
    tracing::info!(
        endpoint = %vault_cfg.endpoint,
        namespace = %vault_cfg.namespace,
        database = %vault_cfg.database,
        "SurrealDB target",
    );
    let embedder = build_embedder();

    // Build a SurrealPetStore using the same vault config — failure here is
    // non-fatal: the pet will keep its in-memory state and just won't
    // persist across restarts.
    let pet_store: Option<Arc<dyn owl_vault::PetStore>> =
        match owl_vault::SurrealPetStore::connect(vault_cfg.clone().into_surreal()).await {
            Ok(s)  => Some(Arc::new(s) as Arc<dyn owl_vault::PetStore>),
            Err(e) => {
                tracing::warn!(err = %e, "pet store init failed — running ephemerally");
                None
            }
        };
    let pet_id = crate::pet_state::pet_id_for(&workspace_for_state);
    let pet = crate::pet_state::PetState::new(
        pet_id, pet_store, Arc::clone(&app_handle_slot),
    ).await;
    pet.start_decay_task();
    pet.start_mind_task();

    let (memory, ctx_provider, mut tool_vec, store_opt) =
        match try_surreal(vault_cfg, workspace_root.clone(), Arc::clone(&embedder)).await {
            Some((mem, ctx, extra_tools, store)) => {
                tracing::info!("SurrealDB connected — using persistent memory store");
                let mut t = tools_vec;
                t.extend(extra_tools);
                (mem, Some(ctx), t, Some(store))
            }
            None => {
                tracing::error!("SurrealDB unreachable — falling back to in-memory store");
                let mem: Arc<dyn owl_brain::MemoryStore> = Arc::new(InMemoryStore::new());
                (mem, None, tools_vec, None)
            }
        };

    // Auto-classify the model: a 2 B local model gets the short prompt +
    // tight context budgets; cloud / 7 B+ models get the full surface.
    // BrainConfig values still win where they're explicitly set.
    let mut rl_cfg = ReasoningConfig::for_model_id(&tower_cfg.model);
    rl_cfg.max_steps = brain_cfg.max_steps;
    if brain_cfg.memory_context_limit > 0 {
        // Honour the config override only if it's tighter — never make
        // memory bigger than the class allows.
        rl_cfg.memory_context_limit =
            brain_cfg.memory_context_limit.min(rl_cfg.memory_context_limit);
    }
    // R-21: hand the workspace path to the loop so it can mount it into
    // the sandbox for `cargo check`.  Skipped when verify is disabled.
    if verify_sandbox.is_some() {
        rl_cfg.workspace_path = Some(workspace_root.to_string_lossy().into_owned());
    }
    tracing::info!(
        model = %tower_cfg.model,
        class = %rl_cfg.model_class.label(),
        memory_limit = rl_cfg.memory_context_limit,
        tool_result_max = rl_cfg.tool_result_max_bytes,
        "ReasoningConfig auto-tuned for model class",
    );

    // Bootstrap the orchestra registry early — needed so the `spawn_agent`
    // tool registered below can resolve agent profiles by id.  Watcher
    // events propagate live edits while the app runs.
    let (orchestra, orchestra_watcher) =
        bootstrap_orchestra(workspace_for_state.clone()).await;

    // `spawn_agent` (Phase 9) needs both the agent factory + registry.  The
    // factory doesn't exist yet (it's about to be built around the executor
    // we're constructing), so we bind it lazily through a RwLock handle.
    let spawn_factory_handle = crate::spawn_agent_tool::SpawnAgentTool::new_handle();
    {
        let registry: Arc<dyn owl_orchestra::Registry> =
            Arc::clone(&orchestra) as Arc<dyn owl_orchestra::Registry>;
        let spawn_tool = crate::spawn_agent_tool::SpawnAgentTool::new(
            Arc::clone(&spawn_factory_handle),
            registry,
        );
        tool_vec.push(Box::new(spawn_tool));
    }

    // Discover MCP tools from enabled server configs and add them to the pool.
    let mcp_tools = discover_mcp_tools().await;
    tool_vec.extend(mcp_tools);

    // Convert to a HashMap for O(1) dispatch (B-1).
    let tool_map: HashMap<String, Box<dyn owl_armory::NativeTool>> =
        tool_vec.into_iter().map(|t| (t.name().to_string(), t)).collect();
    let tools_arc: Arc<HashMap<String, Box<dyn owl_armory::NativeTool>>> = Arc::new(tool_map);

    // Wrap the bare ArmoryExecutor with FilteredExecutor + per-tool policy +
    // an InteractiveGate so destructive tools (bash/write_file/edit_file/...)
    // pause for user approval before running (R-21 spirit).
    let bare: Arc<dyn ToolExecutor> = Arc::new(ArmoryExecutor { tools: Arc::clone(&tools_arc) });
    let gate: Arc<dyn owl_brain::ApprovalGate> = Arc::new(crate::approval::InteractiveGate::new(
        Arc::clone(&app_handle_slot),
        Arc::clone(&pending_approvals),
    ));
    let executor: Arc<dyn ToolExecutor> = Arc::new(
        owl_brain::FilteredExecutor::new(bare, Vec::<String>::new())
            .with_policy(
                AppState::default_tool_policies(),
                owl_protocol::tools::ToolPolicy::AutoApprove,
                gate,
            ),
    );

    let experience: Option<Arc<dyn owl_protocol::experience::ExperienceStore>> =
        store_opt.as_ref().map(|s| {
            Arc::new(owl_vault::SurrealExperienceStore::new(Arc::clone(s)))
                as Arc<dyn owl_protocol::experience::ExperienceStore>
        });

    // `build_loop!` builds the session runner *and* the AgentFactory from a
    // single concrete model — the model is cheap to clone (rig clients are
    // `Arc`-shaped under the hood).  The factory captures the same model so
    // sub-agents and picker-spawned agents share the session's provider.
    //
    // We clone `executor` / `memory` / `ctx_provider` for the factory rather
    // than moving them, because the runner branch consumes them too.
    macro_rules! build_loop {
        ($model:expr, $provider_label:expr) => {{
            let m = $model;
            let factory_inner = crate::agent_factory::DesktopAgentFactory::new(
                m.clone(),
                Arc::clone(&executor),
                Arc::clone(&memory),
                ctx_provider.clone(),
                brain_cfg.memory_context_limit,
                $provider_label,
                rl_cfg.model_class,
            );
            let factory_inner = match (verify_sandbox.clone(), rl_cfg.workspace_path.clone()) {
                (Some(sb), Some(ws)) => factory_inner.with_sandbox(sb, ws),
                _                    => factory_inner,
            };
            let factory: Arc<dyn owl_brain::AgentFactory> = Arc::new(factory_inner);
            let mut loop_ = ReasoningLoop::new(
                m.clone(), Arc::clone(&executor), Arc::clone(&memory), rl_cfg.clone(),
            );
            if let Some(ctx) = ctx_provider.clone() {
                loop_ = loop_.with_context(ctx);
            }
            if let Some(exp) = experience.clone() {
                loop_ = loop_.with_experience(exp);
            }
            if let Some(sb) = verify_sandbox.clone() {
                loop_ = loop_.with_sandbox(sb);
            }
            if let Some(sw) = skill_writer.clone() {
                loop_ = loop_.with_skill_writer(sw);
            }
            // Hermes-style context compaction — auto-shrinks memory once
            // it grows past `compact_threshold` entries.
            let compactor: Arc<dyn owl_brain::Compactor> =
                Arc::new(owl_brain::LlmCompactor::new(m.clone()));
            loop_ = loop_.with_compactor(compactor);
            // Step-level streaming: every state change / tool call / result
            // is broadcast on the `agent_event` Tauri channel for the UI AND
            // appended to the per-conv `*.events.jsonl` log for resume AND
            // forwarded to the Owl pet so it reacts to tool outcomes.
            let sink: Arc<dyn owl_brain::EventSink> = Arc::new(
                crate::event_sink::TauriEventSink::new(
                    Arc::clone(&app_handle_slot),
                    Arc::clone(&active_conv),
                    Some(pet.clone()),
                )
            );
            loop_ = loop_.with_event_sink(sink);
            let carto: Option<Arc<dyn owl_cartographer::GraphIngester>> =
                store_opt.as_ref().map(|s| {
                    Arc::new(owl_cartographer::Cartographer::new(
                        m.clone(), Arc::clone(&embedder), Arc::clone(s),
                    )) as Arc<dyn owl_cartographer::GraphIngester>
                });
            (Arc::new(loop_) as Arc<dyn AgentRunner>, factory, carto)
        }};
    }

    let model_id = tower_cfg.model.clone();
    let gemini_key = tower_cfg.gemini_api_key.clone();

    // (orchestra + orchestra_watcher already bootstrapped above before the
    //  spawn_agent tool was registered.)

    #[cfg(feature = "ollama")]
    if matches!(tower_cfg.provider, Provider::Ollama) {
        let ocfg = OllamaConfig {
            model:    tower_cfg.model.clone(),
            base_url: std::env::var("OWL_OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
        };
        let (runner, factory, carto) = build_loop!(
            build_ollama_model(ocfg)
                .unwrap_or_else(|e| panic!(
                    "Failed to build Ollama provider: {e}\n\
                     Hint: install Ollama (https://ollama.ai), pull a model \
                     (`ollama pull llama3.2`), and set OWL_TOWER_MODEL to that id."
                )),
            "ollama"
        );
        spawn_factory_handle.write().await.replace(Arc::clone(&factory));
        return RunnerInit {
            runner, tools: tools_arc,
            gemini_api_key: gemini_key, model: model_id,
            workspace: workspace_for_state.clone(),
            store: store_opt,
            embedder,
            orchestra,
            orchestra_watcher,
            factory,
            cartographer: carto,
            pet: pet.clone(),
        };
    }

    #[cfg(feature = "gemini")]
    if matches!(tower_cfg.provider, Provider::Gemini) {
        let gcfg = GeminiConfig {
            model:           tower_cfg.model.clone(),
            embedding_model: tower_cfg.embedding_model.clone(),
            api_key:         tower_cfg.gemini_api_key.clone(),
        };
        let (runner, factory, carto) = build_loop!(
            build_gemini_model(gcfg)
                .unwrap_or_else(|e| panic!(
                    "Failed to build Gemini provider: {e}\n\
                     Hint: set GEMINI_API_KEY in the environment that launched the app. \
                     On macOS, GUI apps don't inherit env vars from your shell — \
                     run `cargo tauri dev` from a terminal where the key is exported, \
                     e.g. `GEMINI_API_KEY=... cargo tauri dev`."
                )),
            "gemini"
        );
        spawn_factory_handle.write().await.replace(Arc::clone(&factory));
        return RunnerInit {
            runner, tools: tools_arc,
            gemini_api_key: gemini_key, model: model_id,
            workspace: workspace_for_state.clone(),
            store: store_opt,
            embedder,
            orchestra,
            orchestra_watcher,
            factory,
            cartographer: carto,
            pet: pet.clone(),
        };
    }

    let (runner, factory, carto) = build_loop!(
        build_claude_model(tower_cfg)
            .unwrap_or_else(|e| panic!(
                "Failed to build LLM provider: {e}\n\
                 Hint: set GEMINI_API_KEY or ANTHROPIC_API_KEY in the environment \
                 the app was launched from. On macOS, GUI apps don't see env vars \
                 from your shell config — try `cargo tauri dev` from a terminal \
                 where the key is exported."
            )),
        "anthropic"
    );
    spawn_factory_handle.blocking_write().replace(Arc::clone(&factory));
    RunnerInit {
        runner, tools: tools_arc,
        gemini_api_key: gemini_key, model: model_id,
        workspace: workspace_root,
        store: store_opt,
        embedder,
        orchestra,
        orchestra_watcher,
        factory,
        cartographer: carto,
        pet,
    }
}

/// Initialise the orchestra registry from disk + spawn a hot-reload watcher.
///
/// Returns the registry plus a handle holding the watcher; dropping the
/// handle stops live updates.  Errors are non-fatal — we log and return an
/// empty registry so the app still launches when `.knight-owl/` is missing.
async fn bootstrap_orchestra(
    workspace: PathBuf,
) -> (
    Arc<owl_orchestra::InMemoryRegistry>,
    Arc<tokio::sync::Mutex<Option<owl_orchestra::OrchestraWatcher>>>,
) {
    use owl_orchestra::{FsLoader, InMemoryRegistry, Loader, OrchestraRoots, OrchestraWatcher};

    let registry: Arc<InMemoryRegistry> = Arc::new(InMemoryRegistry::new());
    let loader:   Arc<dyn Loader>       = Arc::new(FsLoader);
    let roots = OrchestraRoots::discover(&workspace);

    let watcher = match OrchestraWatcher::spawn(roots, Arc::clone(&loader) as _, Arc::clone(&registry)).await {
        Ok(w)  => Some(w),
        Err(e) => {
            tracing::warn!(err = %e, "orchestra watcher failed to start; live reload disabled");
            None
        }
    };

    (registry, Arc::new(tokio::sync::Mutex::new(watcher)))
}

/// Attempt to connect to SurrealDB and build memory + context provider.
async fn try_surreal(
    cfg: VaultConfig,
    workspace_root: PathBuf,
    embedder: Arc<dyn owl_vault::Embedder>,
) -> Option<(
    Arc<dyn owl_brain::MemoryStore>,
    Arc<dyn ContextProvider>,
    Vec<Box<dyn owl_armory::NativeTool>>,
    Arc<dyn owl_vault::HybridStore>,
)> {
    let surreal_cfg = cfg.into_surreal();
    let store = match owl_vault::SurrealStore::connect(surreal_cfg.clone()).await {
        Ok(s) => Arc::new(s) as Arc<dyn owl_vault::HybridStore>,
        Err(e) => {
            warn!(err = %e, "SurrealDB connect failed");
            return None;
        }
    };

    // Stable session id per workstation user — keeps memory persistent
    // across app restarts.  TODO: per-conversation IDs once UI tracks them.
    let session_id = "knight-owl-desktop".to_string();
    let mem = match SurrealMemoryStore::connect(surreal_cfg, session_id.clone(), Some(Arc::clone(&embedder))).await {
        Ok(m) => Arc::new(m) as Arc<dyn owl_brain::MemoryStore>,
        Err(e) => {
            warn!(err = %e, "SurrealMemoryStore connect failed");
            return None;
        }
    };

    let graph_ctx: Arc<dyn ContextProvider> = Arc::new(GraphContextProvider {
        store:           Arc::clone(&store),
        embedder:        Arc::clone(&embedder),
        // Phase G — share the same MemoryStore for cross-session recall.
        session_memory:  Some(Arc::clone(&mem) as Arc<dyn owl_protocol::memory::MemoryStore>),
        current_session: session_id,
    });
    // Wrap with ProjectContextProvider so EVERY turn prepends `<project_stack>`
    // + CLAUDE.md framing — the model sees what kind of project this is
    // before any user transcript.  Was previously inlined into the user msg
    // which polluted memory recall on subsequent turns.
    let ctx: Arc<dyn ContextProvider> = Arc::new(ProjectContextProvider {
        workspace: workspace_root.clone(),
        inner: Some(graph_ctx),
    });

    let search_tool: Box<dyn owl_armory::NativeTool> =
        Box::new(SearchCodeTool { store: Arc::clone(&store) });

    let _ = workspace_root; // used by build_registry_with_root above
    Some((mem, ctx, vec![search_tool], store))
}

/// Derive a stable, filesystem-safe id for a workspace from its absolute path.
///
/// Format: `<basename>-<8-char-hex>` — readable in the UI and unique across
/// homonyms (two unrelated repos called `app/` get different ids).
pub fn workspace_id_for(path: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.display().to_string();

    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    let hex = format!("{:08x}", (h.finish() as u32));

    let base = canonical.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_"))
        .unwrap_or_else(|| "ws".to_string());
    format!("{base}-{hex}")
}

/// Determine workspace root: env var → `~/.knight-owl/config.json` → CWD.
fn workspace_root() -> PathBuf {
    if let Ok(p) = std::env::var("OWL_WORKSPACE") {
        return PathBuf::from(p);
    }
    if let Some(p) = read_config().and_then(|c| c.workspace) {
        let pb = PathBuf::from(&p);
        if pb.is_dir() { return pb; }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Persistent app config stored at `~/.knight-owl/config.json`.
///
/// This file tracks the WORKSPACE list only (recent picks shown in
/// sidebar).  All runtime settings (provider, model, API keys, vault
/// credentials, ollama, brain limits, sandbox toggles) live in
/// `~/.knight-owl/settings.json` — see [`crate::commands::settings`].
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    /// Active workspace (absolute path).
    pub workspace: Option<String>,
    /// Recently-used workspaces, most-recent first.
    #[serde(default)]
    pub workspaces: Vec<String>,
}

pub fn config_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".knight-owl");
        p.push("config.json");
        return p;
    }
    PathBuf::from("knight-owl-config.json")
}

pub fn read_config() -> Option<AppConfig> {
    let raw = std::fs::read_to_string(config_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn write_config(cfg: &AppConfig) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, json)
}

// ── Embedder builder ───────────────────────────────────────────────────────

fn build_embedder() -> Arc<dyn owl_vault::Embedder> {
    // 1. Prefer Gemini when its key is present — best semantic quality.
    #[cfg(feature = "gemini")]
    {
        use owl_tower::adapters::gemini::{build_gemini_embedder, GeminiConfig};
        let gcfg = GeminiConfig::load();
        if gcfg.api_key.is_some() || std::env::var("GEMINI_API_KEY").is_ok() {
            match build_gemini_embedder(gcfg) {
                Ok(model) => {
                    tracing::info!("using Gemini embedding model");
                    return Arc::new(owl_vault::RigEmbedder::new(model));
                }
                Err(e) => {
                    tracing::warn!(err = %e, "Gemini embedder failed, falling back");
                }
            }
        }
    }

    // 2. Try local Ollama — `nomic-embed-text` (or `OWL_OLLAMA_EMBED_MODEL`).
    //    User must have `ollama pull nomic-embed-text` first.  We probe the
    //    daemon to avoid building an embedder that will fail every call.
    #[cfg(feature = "ollama")]
    {
        let base_url = std::env::var("OWL_OLLAMA_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:11434".into());
        // Cheap reachability check via std TCP — no async needed here.
        use std::net::ToSocketAddrs;
        let host_port = base_url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/');
        let addr = ToSocketAddrs::to_socket_addrs(host_port)
            .ok()
            .and_then(|mut a| a.next())
            .unwrap_or_else(|| std::net::SocketAddr::from(([127, 0, 0, 1], 11434)));
        let reachable = std::net::TcpStream::connect_timeout(
            &addr,
            std::time::Duration::from_millis(400),
        ).is_ok();
        if reachable {
            match owl_tower::adapters::ollama::build_ollama_embedder(&base_url, None) {
                Ok(model) => {
                    let label = std::env::var("OWL_OLLAMA_EMBED_MODEL")
                        .unwrap_or_else(|_| "nomic-embed-text".into());
                    tracing::info!(model = %label, "using local Ollama embedding model");
                    return Arc::new(owl_vault::RigEmbedder::new(model));
                }
                Err(e) => {
                    tracing::warn!(err = %e, "Ollama embedder build failed");
                }
            }
        } else {
            tracing::debug!(base_url, "ollama daemon not reachable; skipping ollama embedder");
        }
    }

    // 3. Last resort — local hash embedder.  Useless for semantic search but
    //    keeps the rest of the pipeline structurally sound (right vector dim).
    tracing::warn!(
        "using local HashEmbedder — semantic recall WILL BE POOR. \
         Set GEMINI_API_KEY or `ollama pull nomic-embed-text` for real embeddings."
    );
    Arc::new(HashEmbedder::default())
}

// ── ProjectContextProvider — workspace metadata + CLAUDE.md ────────────────

/// Prepends `<project_stack>` + CLAUDE.md/README on every turn so the brain
/// has framing context (Rust? Node? Python?) without polluting the user-
/// message memory transcript.  Stateless — re-reads from disk on each call
/// so edits take effect immediately.
struct ProjectContextProvider {
    workspace: PathBuf,
    /// Optional downstream provider (graph-RAG); we chain ours then theirs.
    inner: Option<Arc<dyn ContextProvider>>,
}

#[async_trait]
impl ContextProvider for ProjectContextProvider {
    async fn retrieve(&self, prompt: &str) -> Result<Vec<String>, BrainError> {
        let mut out: Vec<String> = Vec::new();
        if let Some(ctx) = crate::commands::project_overview::read_project_context(&self.workspace) {
            out.push(ctx);
        }
        if let Some(inner) = &self.inner {
            out.extend(inner.retrieve(prompt).await?);
        }
        Ok(out)
    }
}

// ── GraphContextProvider ────────────────────────────────────────────────────

/// Bridge to WF-13 hybrid retrieval — see `owl_vault::hybrid_retrieve`.
///
/// Both `owl-cli` and `owl-desktop` use the same retrieval implementation
/// from `owl-vault`; this is a thin `ContextProvider` adapter that also
/// folds in Phase G cross-session memory recall when wired.
struct GraphContextProvider {
    store:           Arc<dyn owl_vault::HybridStore>,
    embedder:        Arc<dyn owl_vault::Embedder>,
    /// Optional Phase G recall — set when persistent memory is online.
    session_memory:  Option<Arc<dyn owl_protocol::memory::MemoryStore>>,
    current_session: String,
}

#[async_trait]
impl ContextProvider for GraphContextProvider {
    async fn retrieve(&self, prompt: &str) -> Result<Vec<String>, BrainError> {
        let mut snippets = owl_vault::hybrid_retrieve(
            &*self.store,
            Some(&*self.embedder),
            prompt,
            &owl_vault::HybridRetrieveConfig::default(),
        )
        .await
        .map_err(|e| BrainError::ContextRetrieval(e.to_string()))?;

        if let Some(mem) = &self.session_memory {
            let hits = mem
                .search_session_memory(prompt, &self.current_session, 5)
                .await
                .map_err(|e| BrainError::ContextRetrieval(e.to_string()))?;
            if !hits.is_empty() {
                let body = hits
                    .iter()
                    .map(|h| format!(
                        "[session:{} role:{}] {}",
                        &h.session_id.chars().take(8).collect::<String>(),
                        h.role,
                        h.content.chars().take(200).collect::<String>(),
                    ))
                    .collect::<Vec<_>>()
                    .join("\n");
                snippets.push(format!("<past_sessions>\n{body}\n</past_sessions>"));
            }
        }

        Ok(snippets)
    }
}

// ── Tool bridges ────────────────────────────────────────────────────────────

use std::collections::HashMap;

use owl_armory::traits::NativeTool;
use crate::tools::mcp_proxy::McpToolProxy;
use crate::tools::search_code::SearchCodeTool;

// ── ArmoryExecutor ──────────────────────────────────────────────────────────

/// Tool executor backed by a name → tool map for O(1) dispatch.
struct ArmoryExecutor {
    tools: Arc<HashMap<String, Box<dyn NativeTool>>>,
}

/// Translate common LLM-emitted tool-name variants to our canonical names.
///
/// Small / mid-sized models often hallucinate plausible-sounding tool
/// names (`list_directory`, `cat`, `ls`, …).  Rather than fail the call,
/// we map them to the real tool when there's an obvious match.
fn canonicalize_tool_name(name: &str) -> &str {
    match name {
        // list_dir
        "list_directory" | "listdir" | "list_files" | "ls" | "dir" => "list_dir",
        // read_file
        "cat" | "open_file" | "view_file" | "show_file" | "readfile" => "read_file",
        // write_file
        "create_file" | "writefile" | "save_file" => "write_file",
        // edit_file
        "modify_file" | "replace_in_file" | "patch_file" | "str_replace" => "edit_file",
        // search
        "find" | "search" | "regex_search" => "grep",
        // glob
        "find_files" | "list_glob" => "glob",
        // bash
        "shell" | "run_shell" | "exec" | "execute" => "bash",
        other => other,
    }
}

#[async_trait]
impl ToolExecutor for ArmoryExecutor {
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, BrainError> {
        let canonical = canonicalize_tool_name(&call.name).to_string();
        // Rewrite the call so the receiving tool sees a known name in
        // logs + the agent event stream (`tool_calling.call.name`).
        let normalized = ToolCall { name: canonical.clone(), args: call.args };
        let tool = self
            .tools
            .get(&canonical)
            .ok_or_else(|| BrainError::ToolDispatch(format!("unknown tool: {}", normalized.name)))?;
        tool.run(normalized).await.map_err(|e| BrainError::ToolDispatch(e.to_string()))
    }

    fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(canonicalize_tool_name(name))
    }
}

// ── MCP tool discovery ──────────────────────────────────────────────────────

/// On-disk path for the persisted MCP server config list.
///
/// Uses the same `~/.knight-owl/` base as the rest of the app config so
/// [`build_runner`] can locate it without a Tauri `AppHandle`.
fn mcp_servers_path() -> PathBuf {
    let mut base = if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home)
    } else {
        PathBuf::from(".")
    };
    base.push(".knight-owl");
    base.push("mcp_servers.json");
    base
}

/// Load enabled MCP server configs, connect to each, and return proxies for
/// all discovered tools.
///
/// Errors (bad config, failed spawn, network issues) are logged and skipped —
/// a broken MCP server must never prevent the app from starting.
async fn discover_mcp_tools() -> Vec<Box<dyn owl_armory::NativeTool>> {
    use std::sync::Arc;

    let raw = match std::fs::read_to_string(mcp_servers_path()) {
        Ok(r)  => r,
        Err(_) => return vec![], // no config file yet — fine
    };
    let configs: Vec<owl_protocol::ipc::McpServerConfig> = match serde_json::from_str(&raw) {
        Ok(v)  => v,
        Err(e) => {
            tracing::warn!(err = %e, "failed to parse mcp_servers.json; skipping MCP tools");
            return vec![];
        }
    };

    let mut proxies: Vec<Box<dyn owl_armory::NativeTool>> = vec![];
    for cfg in configs.into_iter().filter(|c| c.enabled) {
        let client = match owl_mcp::connect_server(&cfg).await {
            Ok(c)  => Arc::new(c),
            Err(e) => {
                tracing::warn!(server = %cfg.name, err = %e, "MCP server connection failed; skipping");
                continue;
            }
        };
        let tool_defs = match client.list_tools().await {
            Ok(t)  => t,
            Err(e) => {
                tracing::warn!(server = %cfg.name, err = %e, "MCP tools/list failed; skipping");
                continue;
            }
        };
        let count = tool_defs.len();
        for def in tool_defs {
            proxies.push(Box::new(McpToolProxy::new(
                def.name,
                def.description,
                Arc::clone(&client),
            )));
        }
        tracing::info!(server = %cfg.name, tools = count, "MCP server connected");
    }
    proxies
}
