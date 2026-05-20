# Knight-Owl — Claude Agent Configuration

## Project Identity

Rust workspace for a modular, multi-LLM AI agent framework using the `rig` crate.
Stack: Rust (primary), Tauri v2, React/Next.js (desktop UI only).

## Architecture Map (source of truth)

```
                        ┌─────────────────────┐
         User Input ──► │  owl-cli / owl-desktop│ (apps — anyhow, no thiserror)
                        └──────────┬──────────┘
                                   │ calls
                        ┌──────────▼──────────┐
                        │      owl-brain       │ ◄── The Orchestrator
                        │  loop.rs + memory.rs │     depends on traits only
                        └──┬──────────────┬───┘
              LLM calls    │              │  Tool calls
              ┌────────────▼──┐      ┌───▼────────────┐
              │  owl-tower    │      │   owl-armory    │
              │  adapters/    │      │   tools/        │
              │  prompt.rs    │      │   traits.rs     │
              └───────────────┘      └────────────────┘
                                            │  MCP calls
                                     ┌──────▼──────────┐
                                     │    owl-mcp       │
                                     │  client.rs       │
                                     │  transport/      │
                                     └─────────────────┘
         All crates ──► owl-protocol (shared types, errors, events — zero deps)
         All tests  ──► owl-harness  (mocks, evaluator — test-only)
```

### Nervous System — 4-layer Knowledge Graph (SurrealDB)

All layers live in the same `owl-vault::SurrealStore`. Each layer adds tables + edges
to the hybrid graph; agents traverse them via `HybridStore` + SurrealQL.

```
┌──────────────────────────────────────────────────────────────┐
│ L4 — Experience  : commit, pr, test_run, memory, insight      │ ← owl-harness feeds
│                    edges: PRODUCED, RESOLVED, VIOLATES        │
├──────────────────────────────────────────────────────────────┤
│ L3 — Semantic    : doc, code_chunk (+ embedding)              │ ← owl-cartographer
│                    edges: DESCRIBES, SIMILAR_TO               │    + BM25 index
├──────────────────────────────────────────────────────────────┤
│ L2 — Logic       : symbol (LSP-resolved)                      │ ← owl-cortex (LSP)
│                    edges: REFERENCES, IMPLEMENTS, OVERRIDES   │
├──────────────────────────────────────────────────────────────┤
│ L1 — Syntax      : file, code_node (Function/Class/Variable)  │ ← owl-cortex (tree-sitter)
│                    edges: DEFINES, CALLS, IMPORTS, CONTAINS   │    + DEFINE EVENT triggers
└──────────────────────────────────────────────────────────────┘
                         │
                         ▼
                `standard_node` (SOLID / KISS / DRY / project rules)
                 edges: VIOLATES (from code_node → standard_node)
```

Traversal rule: agents locate → plan → act by **walking the graph** (`SELECT ... FROM
symbol WHERE ->REFERENCES->code_node.path = $file`), not by scanning files.

### Agentic Workflow — 5 phases (embedded in `owl-brain`, not prompts)

| Phase      | Internal action                                                 | Tooling                   |
|------------|-----------------------------------------------------------------|---------------------------|
| Locate     | Graph traversal from entry symbol to impacted code_nodes         | SurrealQL `->` operator   |
| Plan       | Pull relevant `memory` rows (past successes/failures)            | `HybridStore::recall()`   |
| Execute    | Run edits in sandbox, run test harness                           | `owl-sandbox` (Docker)    |
| Review     | Cross-check code_node against `standard_node` violations         | tree-sitter + linter      |
| Learn      | Persist outcome + derive `insight` via distillation              | SurrealDB `DEFINE EVENT`  |

Each phase is a state transition in `owl-brain::loop` — **no phase is prompt-driven**.

### Crate Dependency Graph (enforced — violations are forbidden)

```
owl-protocol   →  (none)
owl-brain      →  owl-protocol, rig-core
owl-tower      →  owl-protocol, rig-core
owl-armory     →  owl-protocol, rig-core
owl-mcp        →  owl-protocol
owl-vault      →  owl-protocol, rig-core, surrealdb
owl-cartographer → owl-protocol, owl-vault, rig-core
owl-cortex     →  owl-protocol, owl-vault, tree-sitter, tower-lsp
owl-sandbox    →  owl-protocol, bollard (Docker), tokio
owl-harness    →  owl-protocol, owl-brain, owl-tower, owl-armory, owl-mcp, owl-sandbox  [dev/test only]
owl-cli        →  owl-brain, owl-protocol, anyhow
owl-desktop    →  owl-brain, owl-protocol, anyhow  (src-tauri only)
```

Cross-crate rule: **owl-brain MUST NOT import owl-armory or owl-tower directly.**
Data flows via trait objects injected at startup (`Arc<dyn CompletionModel>`, `Arc<dyn ToolExecutor>`).

---

## AGENTS

### Agent: `owl-protocol-agent`
**Trigger:** Any change to `shared/owl-protocol/`.
**Responsibility:** Guardian of the shared contract — zero upstream dependencies allowed.
**Behavior:**
- `owl-protocol` has **no** dependencies except `serde`, `thiserror`, `schemars` — enforce this.
- Every new type must derive: `Debug`, `Clone`, `serde::Serialize`, `serde::Deserialize`.
- Event types additionally derive `schemars::JsonSchema` (used by rig tool definitions).
- Never add business logic to `owl-protocol` — only types, error enums, and From impls.
- When adding an error variant, check if an existing variant can be reused or extended.
- Changing an existing type is a **breaking change** — check all downstream crates before proceeding.

### Agent: `owl-brain-agent`
**Trigger:** Any change to `crates/owl-brain/` — loop.rs, memory.rs, or new modules.
**Responsibility:** The Orchestrator — coordinates without knowing concrete tools or models.
**Behavior:**
- `owl-brain` must compile with **only mock impls** of `CompletionModel` and `ToolExecutor`.
  Run: `cargo test -p owl-brain` before any `owl-tower` or `owl-armory` is involved.
- `loop.rs` is a state machine: `Idle → Planning → Acting → Observing → Idle`.
  State enum lives in `owl-protocol::state::AgentState` — never define state locally in owl-brain.
- `memory.rs` exposes only the `MemoryStore` trait. Concrete backends (in-memory, vector) are
  injected — owl-brain never constructs a memory backend directly.
- Reasoning loop steps:
  1. Receive prompt → update short-term memory
  2. Build context from MemoryStore
  3. Call CompletionModel (via rig)
  4. Parse tool call from response
  5. Dispatch to ToolExecutor
  6. Record observation → repeat or return
- Max recursion depth is configurable via `ReasoningConfig` — never hardcode.

### Agent: `owl-tower-agent`
**Trigger:** Any change to `crates/owl-tower/` — adapters, prompt.rs.
**Responsibility:** The Watchtower — normalizes all LLM providers behind rig traits.
**Behavior:**
- Each provider lives in `adapters/<provider>.rs` and is gated with `#[cfg(feature = "<provider>")]`.
- Adapter modules expose: one config struct, one builder, one type implementing `rig::providers::*`.
- `prompt.rs` owns all system prompt templates — no prompt strings anywhere else in owl-tower.
- Provider SDK types (e.g., `anthropic::Client`) must not be `pub` — wrap them entirely.
- Feature flags: `claude` (default), `gemini`, `ollama` — Claude is the default active provider.
- When switching providers, only `Cargo.toml` features change — no code changes in owl-brain.

### Agent: `owl-armory-agent`
**Trigger:** Any change to `crates/owl-armory/` — tools, traits, registry.
**Responsibility:** The Arsenal — every native tool follows the same contract.
**Behavior:**
- All tools implement both `rig::tool::Tool` (for rig agent integration) and
  `owl-armory::traits::NativeTool` (for internal registration and metadata).
- `registry.rs` is the single file that knows all tools — it returns a `Vec<Box<dyn NativeTool>>`.
  Adding a tool = one `push` in registry.rs, nothing else.
- Tool inputs/outputs are types from `owl-protocol::tools` — never define them inside owl-armory.
- Tools are **pure functions** over I/O — no internal state, no Arc fields unless truly needed.
- Shell tool: never use `std::process::Command` with un-sanitized user input. Validate args.
- File tool: restrict paths to an allowed root — no absolute path escapes.

### Agent: `owl-mcp-agent`
**Trigger:** Any change to `crates/owl-mcp/` — client.rs, transport/.
**Responsibility:** The Squire — connects to external MCP servers transparently.
**Behavior:**
- `client.rs` discovers tools from an MCP server and wraps them as `rig::tool::Tool` impls
  so `owl-brain` sees no difference between native tools and MCP tools.
- Transport abstraction: `transport/mod.rs` defines a `McpTransport` trait with:
  `async fn send(&self, msg: McpMessage) -> Result<McpMessage, McpError>`
  Concrete impls: `StdioTransport`, `WsTransport`, `HttpTransport` — each in its own file.
- Message types (`McpMessage`, `McpError`) live in `owl-protocol::mcp`.
- Client reconnect logic lives in `client.rs` only — transports do not retry.
- Never block the tokio runtime in transport read loops — use `tokio::io::AsyncBufReadExt`.

### Agent: `owl-vault-agent`
**Trigger:** Any change to `crates/owl-vault/` — surreal.rs, embedder.rs, store.rs.
**Responsibility:** The Vault — hybrid graph + vector store for semantic memory.
**Behavior:**
- `store::HybridStore` trait is the only API consumers should use; concrete backends
  (SurrealDB, in-memory) are injected as `Arc<dyn HybridStore>`.
- `HybridStore` unifies vector ops (`upsert_documents`, `vector_search`) and graph
  ops (`upsert_entity`, `upsert_relation`, `neighbours`) behind one contract — no
  separate graph store exists.
- `embedder::Embedder` wraps any `rig::embeddings::EmbeddingModel` — never call
  provider SDKs directly inside owl-vault.
- Document / match / entity / relation types come from `owl-protocol` — never
  define them here.
- SurrealDB client (`surrealdb::Surreal<Any>`) is wrapped — never re-exported.
- Schema bootstrap happens in `SurrealStore::connect` via `DEFINE TABLE IF NOT EXISTS`
  and `DEFINE TABLE ... TYPE RELATION FROM entity TO entity`; runtime calls must
  not redefine tables.
- Allowed deps: `owl-protocol`, `rig-core`, `surrealdb`. Never `owl-brain` /
  `owl-tower` / `owl-armory`.
- **Nervous-system schema (owned here, populated by cortex/cartographer/harness):**
  - L1 tables: `file`, `code_node` — edges `DEFINES`, `CALLS`, `IMPORTS`, `CONTAINS`
  - L2 table:  `symbol`         — edges `REFERENCES`, `IMPLEMENTS`, `OVERRIDES`
  - L3 tables: `doc`, `code_chunk` (with `embedding`) — edges `DESCRIBES`, `SIMILAR_TO`
  - L4 tables: `commit`, `pr`, `test_run`, `memory`, `insight` — edges `PRODUCED`,
                `RESOLVED`, `VIOLATES`
  - Standards: `standard_node` (SOLID, KISS, DRY, project rules) — edge `VIOLATES`
- Schema bootstrap uses `DEFINE TABLE IF NOT EXISTS` + `DEFINE TABLE ... TYPE
  RELATION FROM X TO Y`. `DEFINE EVENT` hooks (e.g. `file` upsert → cortex ingest)
  live here, never in downstream crates.
- `HybridStore` gains graph-walk helpers (`impacted_nodes`, `recall_memory`,
  `violations_for`) — all backed by SurrealQL `->` traversal, not Rust loops.

### Agent: `owl-cartographer-agent`
**Trigger:** Any change to `crates/owl-cartographer/` — extract.rs, cartographer.rs, query.rs, prompt.rs.
**Responsibility:** GraphRAG — entity/relation extraction and graph-aware retrieval.
**Behavior:**
- Pipeline is fixed:  text → `extract::extract` (LLM call) → embed entity descriptions
  → `owl_vault::HybridStore::upsert_entity` + `upsert_relation` (graph + vectors
  persist in one DB — no in-memory graph).
- All extraction prompts live in `prompt.rs` — no prompt strings elsewhere in the crate.
- Entity / relation types come from `owl-protocol::graph` — never define them locally.
- LLM calls go through `rig::agent::AgentBuilder` + `Prompt::prompt` (R-11).
- Allowed deps: `owl-protocol`, `owl-vault`, `rig-core`. Never `owl-brain`
  / `owl-tower` / `owl-armory`.
- Hybrid retrieval (`query::query`) must always seed via vector search before walking
  the graph — never walk first.

### Agent: `owl-cortex-agent`
**Trigger:** Any change to `crates/owl-cortex/` — ast.rs, lsp.rs, ingest.rs.
**Responsibility:** The Cortex — L1 Syntax + L2 Logic layers of the nervous system.
**Behavior:**
- Tree-sitter drives L1: file watcher → parse → emit `code_node` rows (`Function`,
  `Class`, `Variable`, `Module`) + edges `DEFINES`, `CALLS`, `IMPORTS`, `CONTAINS`.
- LSP (via `tower-lsp` client) drives L2: resolve cross-file `REFERENCES`,
  `IMPLEMENTS`, `OVERRIDES`. Never duplicate data tree-sitter already provides.
- Persistence is via `owl_vault::HybridStore` — cortex owns no storage of its own.
- Graph updates are idempotent: re-ingesting a file must produce the same node/edge
  set (use content-hash node ids).
- Trigger model: cortex exposes `Ingestor::on_file_changed(path)`; host app wires it
  to fs-watcher OR a SurrealDB `DEFINE EVENT` that fires on `file` upserts.
- Allowed deps: `owl-protocol`, `owl-vault`, `tree-sitter`, `tree-sitter-<lang>`,
  `tower-lsp`. Never `owl-brain` / `owl-tower` / `owl-armory`.
- Language support is gated behind Cargo features: `lang-rust` (default),
  `lang-ts`, `lang-py`. One grammar per feature, loaded in `ast::grammars`.

### Agent: `owl-sandbox-agent`
**Trigger:** Any change to `crates/owl-sandbox/` — docker.rs, runner.rs, harness.rs.
**Responsibility:** The Sandbox — isolated execution + test harness for the
Execute phase of the agentic workflow.
**Behavior:**
- Backed by `bollard` (Docker API) — no shelling out to the `docker` binary.
- One API: `Sandbox::run(plan: ExecutionPlan) -> ExecutionOutcome` where both types
  live in `owl-protocol::sandbox`.
- Execution plans are declarative: image, mounted workspace (read-only by default),
  command vector, resource limits, timeout. No inline bash concatenation.
- Outcomes always carry: exit code, stdout/stderr tail, duration, diff of writable
  volume. Outcomes are persisted as `test_run` rows in SurrealDB (L4 Experience).
- Never grants network access by default — must be explicitly opted-in via
  `ExecutionPlan::allow_network`.
- Allowed deps: `owl-protocol`, `bollard`, `tokio`, `tracing`. Never `owl-brain` /
  `owl-tower` / `owl-armory` / `rig-core` (sandbox never calls LLMs).
- R-5: sandbox exposed to `owl-brain` only via `trait Sandbox` (in `owl-protocol`),
  never the concrete struct.

### Agent: `owl-harness-agent`
**Trigger:** Any change to `crates/owl-harness/` — mock_engine, evaluator.
**Responsibility:** The Training Grounds — all tests use mocks, never live APIs.
**Behavior:**
- `mock_engine.rs` implements `rig::completion::CompletionModel` returning scripted responses.
  Usage: `MockEngine::new().on("prompt", "response").on(...)`.
- `evaluator.rs` scores agent runs: given a prompt and expected tool sequence, compare actual.
  Score = (correct_steps / total_steps). Baseline stored in `owl-harness/baselines/`.
- Every new feature in owl-brain must have a corresponding evaluator test case.
- Mock tools implement both `rig::tool::Tool` and `NativeTool` — same interface as real tools.
- `owl-harness` is a `[dev-dependency]` only — it must never appear in a non-harness crate's
  `[dependencies]`.

### Agent: `owl-desktop-agent`
**Trigger:** Any change to `apps/owl-desktop/` — src-tauri or ui.
**Responsibility:** Desktop bridge — Tauri IPC connects Rust backend to React frontend.
**Behavior:**
- Tauri commands (`#[tauri::command]`) live in `src-tauri/src/commands/` — one file per domain.
- Commands call into `owl-brain` via the same trait interfaces — no business logic in command handlers.
- IPC payload types are defined in `owl-protocol::ipc` — shared between Rust and TypeScript
  (generated via `ts-rs` or `schemars` JSON schema export).
- Frontend state management: React Query for async data, Zustand for UI-only state.
- Streaming LLM responses: use Tauri events (`window.emit`) not command return values.
- Never expose raw `owl-brain` types to the frontend — always serialize through owl-protocol IPC types.

### Agent: `owl-cli-agent`
**Trigger:** Any change to `apps/owl-cli/`.
**Responsibility:** CLI — fast, scriptable, single binary.
**Behavior:**
- Argument parsing via `clap` derive macros — no manual `std::env::args()`.
- One subcommand = one module in `src/commands/`.
- Output: machine-readable mode (`--json`) must always be available alongside human output.
- Uses `anyhow` for error propagation — CLI errors print user-friendly messages, not debug dumps.
- Never link directly to `owl-armory` or `owl-tower` — only `owl-brain` (which injects them).

### Agent: `rust-architect`
**Trigger:** Designing new crates, modules, or cross-crate interfaces.
**Behavior:**
- Reason from domain first, then map to Rust types.
- Define traits before structs — the contract precedes the implementation.
- Enforce crate boundaries: `owl-protocol` owns all shared types; crates never import each other laterally without going through `owl-protocol`.
- Prefer `async_trait` + `Arc<dyn Trait>` for injectable dependencies.
- Return a module tree with `pub use` re-exports at `lib.rs` before writing any logic.

### Agent: `config-prompt-guardian`
**Trigger:** Any change that introduces or edits a configuration value (URL, model id,
timeout, numeric tunable) OR any prompt / LLM instruction string.
**Responsibility:** Enforce R-19 — configs and prompts MUST live outside business logic.
**Behavior:**
- Every crate that reads runtime configuration has exactly one `src/config.rs`
  defining a `Config` struct (`#[derive(Debug, Clone, serde::Deserialize)]`) with a
  `Config::load()` that merges: defaults → `config/<crate>.toml` → env vars.
- No `std::env::var(...)` calls outside `config.rs`.
- Every crate that calls an LLM has either:
  - `src/prompt.rs` holding `pub const <NAME>_SYSTEM: &str` + `pub fn <name>_user(...)`
    builders — strings only, no logic, OR
  - `prompts/<name>.md` files loaded via `include_str!` from `prompt.rs`.
- When a prompt grows beyond ~20 lines or has variants → externalize to
  `prompts/<name>.md`. Keep the loader thin.
- Forbid inline prompt strings in: `reasoning_loop.rs`, `cartographer.rs`,
  `extract.rs` business flow, any tool `call()` body, any Tauri command.
- When reviewing a diff: flag every string literal ≥ 40 chars that looks like an
  instruction (starts with "You are", "Return", "Given", contains `\n`) sitting
  outside `prompt.rs`/`prompts/`.
- Flag every hardcoded URL, port, path, model id, timeout, or tunable default
  sitting in a function body — must move to `Config`.

### Agent: `rust-implementer`
**Trigger:** Writing function bodies, struct impls, tool handlers.
**Behavior:**
- One function = one action. If a function needs an "and" to describe it, split it.
- Mandatory signature review: correct lifetimes, `impl Trait` vs `dyn Trait`, `Send + Sync` bounds.
- Errors: define domain-specific `thiserror` enums; never `unwrap()` or `expect()` in library code.
- Async: use `tokio`; avoid `block_on` inside async contexts.
- After writing: run the KISS check — can this be shorter without losing clarity?

### Agent: `rust-reviewer`
**Trigger:** Any `/review` command or before finalizing a PR diff.
**Behavior:**
- Check SOLID, KISS, DRY violations (see RULES section).
- Flag any `clone()` that could be a reference, any `unwrap()`, any function > 30 lines.
- Verify `rig` usage follows the patterns in WORKFLOWS.
- Output structured feedback: `[VIOLATION]`, `[SUGGESTION]`, `[OK]`.

### Agent: `rig-specialist`
**Trigger:** Any code touching `rig-core`, agents, completions, embeddings, vector stores.
**Behavior:**
- Always use `rig::completion::Prompt` or `rig::agent::Agent` — never raw HTTP calls to LLM APIs.
- Chain tools via `rig`'s `.tool()` builder — never implement tool dispatch manually.
- Embeddings go through `rig::embeddings::EmbeddingsBuilder`.
- Vector store integrations use `rig::vector_store::VectorStoreIndex` trait.
- Validate that every `rig::agent::Agent` has a system prompt injected at build time.

---

## RULES

### R-1 — Single Responsibility (SOLID: S)
```
ENFORCE: Each function does exactly one thing.
ENFORCE: Each struct/enum represents exactly one concept.
ENFORCE: Each module owns one domain boundary.
DETECT: Functions with "and", "or", "also" in their name → split.
DETECT: Functions > 30 lines → refactor.
```

### R-2 — Open/Closed (SOLID: O)
```
ENFORCE: Extend behavior via new trait implementations, not by modifying existing impls.
ENFORCE: Tool registration in owl-armory uses a registry/builder pattern — adding a tool
         must not touch existing tool code.
DETECT: match arms on concrete types that should be trait dispatch → refactor to trait.
```

### R-3 — Liskov Substitution (SOLID: L)
```
ENFORCE: Every type implementing a trait must fulfill the full contract — no panicking stubs.
ENFORCE: Mock types in owl-harness must be drop-in replacements for real types.
DETECT: trait impl methods that panic/todo!/unimplemented! in non-test code → block.
```

### R-4 — Interface Segregation (SOLID: I)
```
ENFORCE: Traits have the minimum viable method set. Split fat traits.
ENFORCE: owl-tower provider traits: separate Completion, Embedding, Streaming — never one god trait.
DETECT: Trait with > 5 methods where callers only use 1-2 → split candidate.
```

### R-5 — Dependency Inversion (SOLID: D)
```
ENFORCE: High-level modules (owl-brain) depend on traits, not concrete types.
ENFORCE: Inject dependencies via constructor — no global singletons except tracing subscriber.
ENFORCE: owl-brain must compile without owl-armory or owl-tower in scope (test with mock impls).
```

### R-6 — KISS
```
ENFORCE: No abstraction layer that isn't immediately used in ≥2 places.
ENFORCE: Prefer std before adding a new crate dependency.
ENFORCE: If a type alias makes the code harder to read, remove it.
DETECT: Nested generics > 2 levels deep → simplify or introduce a newtype.
DETECT: Trait bounds list > 4 items → wrap in a supertrait.
```

### R-7 — DRY
```
ENFORCE: Shared types live in owl-protocol — never duplicate a struct across crates.
ENFORCE: Repeated error-mapping logic → extract to a From<> impl or a helper in owl-protocol.
ENFORCE: Repeated async retry logic → one function in owl-brain::utils.
DETECT: Identical code blocks appearing ≥2 times → extract.
```

### R-8 — Naming Conventions
```
Types/Traits:    PascalCase          (AgentLoop, ToolResult)
Functions:       snake_case          (run_loop, handle_error)
Constants:       SCREAMING_SNAKE     (MAX_RETRIES, DEFAULT_TIMEOUT)
Modules:         snake_case          (mod memory; mod transport;)
Crates:          kebab-case          (owl-brain, owl-armory)
Lifetimes:       single letter       ('a, 'ctx)  — descriptive only if >2 in scope ('store, 'req)
Generics:        PascalCase, semantic (M: Model, T: Tool, E: Embedding)
```

### R-9 — Error Handling
```
ENFORCE: Library crates define their own error enum with thiserror.
ENFORCE: Every error variant carries enough context to debug without logs.
ENFORCE: Application layer (owl-cli, owl-desktop) uses anyhow for ergonomic propagation.
ENFORCE: No .unwrap() / .expect() outside of test code and main() startup assertions.
ENFORCE: ? operator preferred over explicit match for error propagation.
```

### R-10 — Comments
```
ENFORCE: No comments that restate what the code says.
ENFORCE: Comments explain WHY (invariant, constraint, non-obvious decision).
ENFORCE: Doc comments (///) on every pub fn, pub struct, pub trait.
ENFORCE: SAFETY: comment required above every unsafe block explaining the invariant.
ENFORCE: When a function reads a config field, the doc comment must name the
         source key — e.g. `/// Uses `Config::endpoint` (config/owl-vault.toml).`.
ENFORCE: Every `prompt.rs` pub const carries a doc comment naming the external
         template file (if any) and the variables it expects.
ENFORCE: Every `config.rs` field has a doc comment stating: purpose, default,
         env-var override name (if any).
FORBID: Commented-out code committed to main.
FORBID: Inline TODOs marking "move this config later" — do it now or don't merge.
```

### R-11 — rig Usage (Mandatory)
```
ENFORCE: All LLM calls go through rig::completion::Prompt or rig::agent::Agent.
ENFORCE: Tool definitions implement rig::tool::Tool — never ad-hoc JSON dispatch.
ENFORCE: Vector search uses rig::vector_store::VectorStoreIndex.
ENFORCE: No direct reqwest/hyper calls to LLM API endpoints.
ENFORCE: Model provider is injected at runtime — hardcoding "claude-3" strings is forbidden
         except in owl-tower adapter configuration.
```

### R-12 — Async & Concurrency
```
ENFORCE: Async runtime is tokio — no mixing with async-std.
ENFORCE: Shared mutable state → Arc<RwLock<T>> or channels (tokio::sync::mpsc).
ENFORCE: No std::sync::Mutex held across .await points.
ENFORCE: Spawned tasks must be named (tokio::task::Builder::new().name(...)).
ENFORCE: Select! branches must be exhaustive — always include a cancellation arm.
```

### R-13 — Crate Dependency Graph (Architecture Boundary)
```
ENFORCE: owl-protocol has zero project-internal dependencies.
ENFORCE: owl-brain imports owl-protocol + rig only — never owl-armory or owl-tower.
ENFORCE: owl-tower imports owl-protocol + rig only — never owl-brain or owl-armory.
ENFORCE: owl-armory imports owl-protocol + rig only — never owl-brain or owl-tower.
ENFORCE: owl-mcp imports owl-protocol only — never owl-brain, owl-tower, or owl-armory.
ENFORCE: owl-vault imports owl-protocol + rig + surrealdb only — never owl-brain.
ENFORCE: owl-cartographer imports owl-protocol + owl-vault + rig only —
         never owl-brain, owl-tower, owl-armory, or owl-mcp.
ENFORCE: owl-harness is a dev-dependency — never appears in [dependencies] of any crate.
ENFORCE: owl-cli and owl-desktop import owl-brain + owl-protocol only (not tower/armory directly).
DETECT: Any Cargo.toml that violates the above graph → block and report.
```

### R-14 — owl-protocol Purity
```
ENFORCE: owl-protocol contains only: types, error enums, From impls, constants.
ENFORCE: No async code, no I/O, no rig calls in owl-protocol.
ENFORCE: Every type in owl-protocol derives Debug + Clone + Serialize + Deserialize.
ENFORCE: Tool input/output types additionally derive JsonSchema (for rig definitions).
ENFORCE: IPC types additionally annotated with #[ts(export)] for TypeScript generation.
DETECT: Any impl block with non-trivial logic in owl-protocol → extract to the owning crate.
```

### R-15 — Data Flow Integrity
```
ENFORCE: All events crossing crate boundaries use owl-protocol types — never raw strings or serde_json::Value.
ENFORCE: owl-brain receives tool results as owl-protocol::ToolResult — never as raw bytes.
ENFORCE: LLM responses are deserialized into owl-protocol::CompletionResponse before owl-brain sees them.
ENFORCE: MCP tool calls arrive at owl-brain as owl-protocol::ToolCall — client.rs is responsible for mapping.
ENFORCE: Tauri IPC payloads are owl-protocol::ipc types — never raw owl-brain structs.
```

### R-16 — App Layer vs Library Layer
```
ENFORCE: owl-cli and owl-desktop (apps) use anyhow — never thiserror.
ENFORCE: All crates under crates/ use thiserror — never anyhow.
ENFORCE: Apps never contain business logic — they parse input, call owl-brain, format output.
ENFORCE: owl-cli output: default human-readable, always support --json for machine output.
ENFORCE: owl-desktop Tauri commands are thin wrappers — max 10 lines each.
DETECT: Business logic in apps/ → move to owl-brain or owl-protocol.
```

### R-17 — MCP Transport Abstraction
```
ENFORCE: McpTransport trait is the only API owl-mcp::client.rs uses — never concrete transport types.
ENFORCE: Transport selection (Stdio/Ws/Http) happens at startup via config — never conditionally inside client.rs.
ENFORCE: Each transport handles only its own protocol — no fallback/retry inside transport impls.
ENFORCE: Retry and reconnect logic lives exclusively in client.rs.
ENFORCE: McpMessage and McpError are owl-protocol types — transports never define their own message types.
```

### R-18 — Reasoning Loop Invariants
```
ENFORCE: owl-brain::loop::ReasoningLoop never calls owl-armory or owl-tower directly.
ENFORCE: The loop dispatches tool calls via Arc<dyn ToolExecutor> — injected at construction.
ENFORCE: The loop calls the LLM via Arc<dyn CompletionModel> — injected at construction.
ENFORCE: AgentState transitions are the only place loop.rs mutates state.
ENFORCE: Memory reads/writes go through Arc<dyn MemoryStore> — loop never accesses memory directly.
ENFORCE: Maximum iteration depth is enforced by ReasoningConfig::max_steps — loop checks it every iteration.
DETECT: Any direct struct construction of a tool or model inside loop.rs → block.
```

### R-19 — Config & Prompt Externalization (MANDATORY)
```
ENFORCE: No hardcoded configuration values inside business-logic code.
         Config lives in dedicated files: `config/*.toml`, `config/*.yaml`, or env vars
         loaded through a single `Config` struct per crate (e.g. `crate::config::Config`).
ENFORCE: No prompt strings (system, user, few-shot, extraction templates) inside .rs
         business-logic files. Prompts live either in:
           (a) a dedicated `prompt.rs` / `prompts/` module that only holds string
               constants and `fn build_*_prompt(...)` formatters, OR
           (b) external template files under `crates/<crate>/prompts/*.md` loaded via
               `include_str!("../prompts/<name>.md")`.
ENFORCE: Every crate that calls an LLM must expose `prompt.rs` OR `prompts/` — never
         both, and never inline.
ENFORCE: Configuration structs derive `serde::Deserialize` and are loaded via
         `figment` / `config` crate / `serde_yaml` — never parsed ad-hoc.
ENFORCE: Secret values (API keys, tokens) are read from env vars only — never from
         committed config files. The config struct uses `Option<String>` + an env
         override layer.
ENFORCE: Tauri `tauri.conf.json`, `rig` model identifiers, endpoint URLs, timeouts,
         retry counts, top_k / depth defaults — all live in config files, not inline.
DETECT: String literal ≥ 40 chars inside .rs that looks like a prompt (contains
        "You are" / "system:" / "\nUser:" / "```json" / "Schema:") → block.
DETECT: Numeric / string literal used as a tunable (max_steps, top_k, url, model
        name, timeout) appearing directly in a function body → move to config.
DETECT: `std::env::var` called outside a `config.rs` module → refactor into the
        crate's config loader.
```

### R-20 — Graph-First Locate (MANDATORY)
```
ENFORCE: Before any edit, owl-brain identifies impacted code via HybridStore graph
         traversal — never by regex / filesystem scan / full-text grep.
ENFORCE: The Locate phase produces a Vec<CodeNodeRef> (owl-protocol::graph) sourced
         from SurrealQL queries using the `->` operator on L1/L2 edges.
ENFORCE: LLM prompts are built FROM the graph result — not the other way around.
         The agent never asks the LLM "which files are impacted?" before consulting
         the graph.
ENFORCE: Hybrid retrieval always combines vector search (L3) AND graph walk (L1/L2).
         Pure vector or pure grep is forbidden for code navigation.
DETECT: `glob::glob` / `walkdir` / `std::fs::read_dir` inside owl-brain or the
        Locate phase → block. Those belong to cortex ingest only.
DETECT: Any prompt that contains "search the codebase for" without a preceding
        graph query → block.
```

### R-21 — Sandbox-Verified Execute (MANDATORY)
```
ENFORCE: Every code edit produced by the agent is verified in owl-sandbox BEFORE
         being reported as complete. No exceptions for "trivial" changes.
ENFORCE: The Execute phase emits a single owl_protocol::sandbox::ExecutionPlan and
         receives an ExecutionOutcome. No direct filesystem writes to the user's
         workspace until the outcome is Success.
ENFORCE: The outcome is persisted as a `test_run` row in SurrealDB (L4) with
         RELATE edges to the originating user request and the modified code_nodes.
ENFORCE: If the sandbox outcome is Failure, owl-brain loops back to Plan with the
         failure attached — never surfaces the failure as the final response.
ENFORCE: Sandbox uses Docker isolation via `bollard`. Running edits on the host FS
         is forbidden outside explicit developer opt-in (e.g. `--no-sandbox` CLI
         flag, never default).
DETECT: `std::fs::write` in owl-brain / owl-armory outside a sandbox-issued handle
        → block.
DETECT: A Reasoning loop that returns Idle without a matching `test_run` row for
        the current task → block.
```

### R-22 — Self-Reflection & Knowledge Distillation
```
ENFORCE: After every task completion (success OR failure), owl-brain runs a
         background reflection step that writes one `memory` row linking:
         user_request -> plan -> actions -> test_run -> outcome.
ENFORCE: The reflection is deterministic — it's a graph INSERT, not an LLM call.
         Semantic summaries may be added later by the distillation job.
ENFORCE: A periodic distillation job (triggered by SurrealDB `DEFINE EVENT` on
         `memory` insert rate) clusters recurring failures into `insight` rows.
         Insights are consulted during the Plan phase.
ENFORCE: `insight` rows are typed: { kind: "pattern" | "anti_pattern" | "rule",
         scope: "global" | "crate:<name>" | "file:<path>", evidence: Vec<memory_id> }.
ENFORCE: The Review phase queries `standard_node` + active `insight` rows and
         blocks completion on any VIOLATES edge created during Execute.
DETECT: Task completion without a corresponding `memory` row → block merge.
DETECT: `insight` rows written by handwritten code paths — they must come from
        the distillation job only.
```

---

## WORKFLOWS

### WF-1 — Implementing a New Tool (owl-armory)

```
1. Define the domain type in owl-protocol (input struct, output enum).
2. Add a new file in owl-armory/src/tools/<tool_name>.rs.
3. Implement rig::tool::Tool for the struct.
4. Implement traits::NativeTool (defined in owl-armory::traits).
5. Register in owl-armory/src/registry.rs — one line, no other file changes.
6. Add unit test using owl-harness::mock_engine.
7. Verify: `cargo test -p owl-armory`, `cargo clippy -p owl-armory -- -D warnings`.
```

### WF-2 — Adding a New LLM Provider (owl-tower)

```
1. Create owl-tower/src/adapters/<provider>.rs.
2. Implement the rig provider traits (CompletionModel, EmbeddingModel if supported).
3. Implement owl-tower::ProviderAdapter for unified config loading.
4. Wire into owl-tower/src/lib.rs registry via a feature flag (e.g., feature = "gemini").
5. Add integration test gated on the feature flag in owl-harness.
6. Update Cargo.toml optional deps — never enable features unconditionally.
```

### WF-3 — Modifying the Reasoning Loop (owl-brain)

```
1. Read owl-brain::loop::ReasoningLoop — understand current state machine.
2. Changes to loop state → update the State enum in owl-protocol first.
3. Memory interactions must go through owl-brain::memory::MemoryStore trait — no direct access.
4. After change: run owl-harness evaluator against the golden test suite.
5. No merge if evaluator score drops below baseline.
```

### WF-4 — Code Review Checklist

```
□ Every new pub item has a doc comment.
□ No unwrap/expect outside tests.
□ Functions ≤ 30 lines.
□ No lateral crate imports (A → B without owl-protocol).
□ New dependencies justified in PR description.
□ rig APIs used correctly (no raw HTTP to LLM).
□ Async: no Mutex across .await, tasks are named.
□ Tests added for new behavior.
□ `cargo clippy -- -D warnings` passes.
□ `cargo fmt --check` passes.
```

### WF-5 — Starting a New Crate

```
1. Add crate under crates/ with `cargo new --lib crates/<name>`.
2. Add to root Cargo.toml [workspace.members].
3. Create src/lib.rs with crate-level doc comment and #![forbid(unsafe_code)] if applicable.
4. Add crate to owl-protocol dependency if shared types are needed.
5. Define the crate's public API in lib.rs before writing any implementation.
6. Create tests/ integration test directory immediately.
```

### WF-6 — Adding an MCP Transport (owl-mcp)

```
Data flow: owl-brain → owl-mcp::Client → McpTransport → external MCP server

1. Define message types in owl-protocol::mcp if not already present.
2. Create crates/owl-mcp/src/transport/<transport_name>.rs.
3. Implement owl-mcp::transport::McpTransport trait:
   - async fn send(&self, msg: McpMessage) -> Result<McpMessage, McpError>
   - async fn close(&self) -> Result<(), McpError>
4. Add variant to owl-mcp::transport::TransportKind enum.
5. Wire into owl-mcp::client::McpClient::connect() via match on TransportKind.
6. Add unit test using a local echo server (no external dependency in tests).
7. Verify: cargo test -p owl-mcp
```

### WF-7 — Adding a Shared Type (owl-protocol)

```
CAUTION: owl-protocol changes affect every crate — check all dependents before merging.

1. Identify which module the type belongs to:
   - owl-protocol::tools     → tool inputs/outputs
   - owl-protocol::mcp       → MCP messages
   - owl-protocol::state     → agent state machine
   - owl-protocol::ipc       → Tauri IPC payloads
   - owl-protocol::error     → error enums
   - owl-protocol::events    → cross-crate events

2. Add the type with full derives:
   #[derive(Debug, Clone, Serialize, Deserialize)]        // always
   #[derive(JsonSchema)]                                  // if used in rig tool definitions
   #[cfg_attr(feature = "ipc", derive(ts_rs::TS))]       // if IPC payload

3. Add From<> impls if this type replaces an existing representation.
4. Run: cargo check --workspace  (must compile all crates)
5. Update owl-harness fixtures if affected.
```

### WF-8 — Adding a Tauri Command (owl-desktop)

```
Data flow: React UI → Tauri invoke → #[tauri::command] → owl-brain → response → React

1. Define IPC payload types in owl-protocol::ipc (input + output structs).
   - If streaming: output is an event name (String), not the data itself.
2. Create command handler in apps/owl-desktop/src-tauri/src/commands/<domain>.rs:
   - #[tauri::command] function, max 10 lines
   - Calls owl-brain via the injected AppState
   - Returns Result<IpcOutput, String> (String error for Tauri serialization)
3. Register command in tauri::Builder in main.rs — one line.
4. For streaming responses: use window.emit("<event>", payload) inside a spawned task.
5. Generate TypeScript types: cargo run -p owl-desktop --bin ts-gen
   (emits owl-protocol::ipc types to apps/owl-desktop/ui/src/types/protocol.ts)
6. Write the React hook in ui/src/hooks/use<Command>.ts using React Query or Tauri invoke.
7. Verify: cargo tauri dev  (smoke test the command end-to-end)
```

### WF-9 — Running Evaluations (owl-harness)

```
Purpose: Verify owl-brain's reasoning quality doesn't regress.

1. Write a test case in crates/owl-harness/tests/eval_<scenario>.rs:
   - Build a MockEngine with scripted LLM responses
   - Build mock tools with expected call sequences
   - Run ReasoningLoop::run() with the test prompt
   - Assert: evaluator.score() >= BASELINE_SCORE

2. Record the baseline:
   cargo test -p owl-harness -- --nocapture > owl-harness/baselines/<scenario>.txt

3. On every PR, CI runs:
   cargo test -p owl-harness
   Score must be >= stored baseline — CI fails if it drops.

4. When intentionally improving the agent:
   Update the baseline file in the same PR as the improvement.
```

### WF-10 — Data Flow Verification (Architecture Health Check)

```
Run this check before any major merge to ensure the architecture is intact.

1. Dependency graph check:
   cargo tree -p owl-brain | grep -E "owl-(armory|tower|mcp|harness)"
   → Must return empty (no direct deps on those crates)

2. Protocol purity check:
   cargo tree -p owl-protocol
   → Must show only: serde, thiserror, schemars, ts-rs (optional)

3. Build isolation test:
   cargo test -p owl-brain --no-default-features
   → Must compile and pass using only mock impls from owl-harness

4. Clippy full workspace:
   cargo clippy --workspace -- -D warnings

5. Format check:
   cargo fmt --all --check
```

### WF-11 — Externalizing Config & Prompts (R-19)

```
Apply this workflow any time you add a tunable value or a new LLM instruction.

A. For a NEW CONFIG VALUE:
   1. Open (or create) crates/<crate>/src/config.rs.
   2. Add the field to the crate's `Config` struct with a default value.
   3. Add the same key under config/<crate>.toml with a documented default.
   4. If sensitive (API key, token): read from env in Config::load(); NEVER commit to TOML.
   5. Replace every inline literal use-site with `config.<field>`.
   6. Verify: grep for the old literal across the crate — must return zero hits.

B. For a NEW PROMPT:
   1. Decide size: ≤ 20 lines → add to prompt.rs as `pub const`. > 20 lines or
      multi-variant → create prompts/<name>.md and `include_str!` it in prompt.rs.
   2. Re-export from prompt.rs only — never from the business module.
   3. Business code calls `prompt::<name>_system()` / `prompt::<name>_user(args)`,
      never a string literal.
   4. Verify: grep for "You are" / "```json" / "Schema:" in src/ outside prompt.rs —
      must return zero hits.

C. Review gate:
   cargo check -p <crate>  &&  cargo clippy -p <crate> -- -D warnings
   Then run the `externalize-config-prompts` skill to audit the diff.
```

### WF-12 — Tree-sitter Ingest (owl-cortex, L1/L2)

```
Data flow: file change → cortex → owl-vault (code_node + edges) → DEFINE EVENT fanout

1. Grammar selection: match file extension → Cargo-feature-gated grammar in
   owl-cortex::ast::grammars. Unknown extension → no-op, never panic.
2. Parse:
   - tree-sitter produces a concrete syntax tree
   - Extract nodes at defined capture points (function_item, impl_item, struct_item, …)
   - Compute node id = blake3(file_path + node_kind + byte_range)
3. Diff against previous ingest for the same file (query by file id), then:
   - Upsert new/changed code_node rows
   - Create/update edges: DEFINES, CALLS, IMPORTS, CONTAINS
   - Delete code_node rows whose hash no longer appears (orphans)
4. Hand off to LSP step (if enabled): for each symbol, call `textDocument/references`
   and materialize REFERENCES / IMPLEMENTS / OVERRIDES edges on L2.
5. SurrealDB `DEFINE EVENT` on `code_node` changes fires:
   - Invalidate any `code_chunk` whose `DESCRIBES` target changed (L3 re-embed)
   - Mark any `memory` row touching the changed node as `stale = true`
6. Verify: cargo test -p owl-cortex  (uses fixture repos under tests/fixtures/)
```

### WF-13 — Hybrid Retrieval (Graph + Vector + BM25)

```
Purpose: Locate phase — answer "what code is relevant to this request?"

1. Input: user prompt + current file (optional).
2. Parallel fetch:
   (a) Vector seed (L3): HybridStore::vector_search(embed(prompt), top_k=8)
   (b) BM25 seed (L3):   HybridStore::keyword_search(tokens(prompt), top_k=8)
                         (SurrealQL `SEARCH @@ ...` or `string::matches`)
   (c) Focus seed (L1):  if file given, all code_nodes CONTAINED by that file
3. Merge seeds → unique code_node id set.
4. Graph expansion (L1/L2): SELECT ->CALLS->code_node, ->REFERENCES->symbol
   up to depth=2 from each seed. Deduplicate.
5. Score:
   combined_score = 0.4 * vector_score + 0.3 * bm25_score + 0.3 * graph_proximity
6. Return top-N as Vec<CodeContextChunk> — each carries: node id, file/range, code
   body, graph path to seed, score.
7. Pass ONLY the ranked chunks to the LLM — never the raw prompt + full files.

Invariants: a pure vector-only path is forbidden (violates R-20). A call without
a graph expansion step is a bug.
```

### WF-14 — Self-Reflection & Knowledge Distillation Loop

```
Triggers: (A) end of every agent task  (B) scheduled distillation job (hourly)

(A) End-of-task reflection — runs synchronously before returning to the user:
    1. Gather: user_request, plan, actions[], test_run outcome.
    2. Build a `memory` row:
       CREATE memory SET
         request   = $req,
         plan      = $plan,
         actions   = $actions,
         outcome   = $outcome,     // Success | Failure { reason, stderr_tail }
         code_refs = $code_node_ids,
         created   = time::now();
    3. RELATE user_request->PRODUCED->memory and memory->TOUCHED->code_node (for each).
    4. If outcome is Failure, also RELATE memory->VIOLATES->standard_node
       for any rule the Review phase flagged.

(B) Distillation job — async, triggered by DEFINE EVENT on memory insert count:
    1. Cluster memory rows by (error_signature, touched_crate).
    2. Any cluster ≥ 3 failures in a rolling 7-day window → upsert an `insight`:
       { kind: "anti_pattern", scope: "crate:<name>", evidence: [memory_ids] }.
    3. Any cluster ≥ 5 successes with the same fix_pattern → `insight` with
       kind = "pattern".
    4. On next Plan phase, owl-brain queries active insights WHERE scope MATCHES
       current task and injects them as constraints (not prose) into the plan.

Guardrails: distillation is code, not LLM. Summarizing insights into prose is a
separate, optional step and MUST NOT write to `insight.kind` or `scope` fields.
```

---

## SKILL DEFINITIONS

### Skill: `implement-tool`
When asked to add a tool, follow WF-1 strictly. Output in this order:
1. Type definition (owl-protocol)
2. Tool struct + rig::Tool impl (owl-armory)
3. Registry entry (one line)
4. Unit test

### Skill: `refactor-to-solid`
When asked to refactor:
1. Identify which SOLID principle is violated.
2. Show the violation with a code comment.
3. Propose the refactored version.
4. Confirm DRY and KISS are not broken by the refactor.

### Skill: `add-provider`
When asked to add an LLM provider, follow WF-2. Always:
- Gate behind a Cargo feature flag.
- Use rig adapter traits exclusively.
- Never expose the provider SDK types beyond the adapter module.

### Skill: `audit-rig-usage`
Scan all files importing `rig`. For each usage:
- `[OK]` if using official rig traits/builders.
- `[VIOLATION]` if bypassing rig (raw HTTP, manual JSON tool dispatch).
- `[SUGGESTION]` if a rig helper exists that would simplify the code.

### Skill: `check-architecture`
When asked to verify architecture health, execute WF-10 mentally against the code:
1. Check owl-brain's Cargo.toml — no owl-armory/tower/mcp in `[dependencies]`.
2. Check owl-protocol's Cargo.toml — no project-internal deps.
3. Verify data crossing crate boundaries uses owl-protocol types, not ad-hoc structs.
4. Verify app-layer code (owl-cli, owl-desktop) contains no business logic.
5. Report each boundary violation with file + line.

### Skill: `add-protocol-type`
When adding a shared type, follow WF-7 strictly:
1. Identify the correct module in owl-protocol (tools/mcp/state/ipc/error/events).
2. Write the type with all mandatory derives.
3. Add From<> impls if replacing an existing representation.
4. Report all downstream crates that need updating.

### Skill: `implement-mcp-transport`
When adding an MCP transport, follow WF-6:
1. Protocol type in owl-protocol::mcp first.
2. McpTransport trait impl in transport/<name>.rs.
3. Wire into TransportKind enum — one match arm, nothing else.
4. Unit test with local echo — no external servers.

### Skill: `add-tauri-command`
When adding a desktop command, follow WF-8:
1. IPC types in owl-protocol::ipc.
2. Thin command handler (≤10 lines) in commands/<domain>.rs.
3. Emit TypeScript types via ts-rs.
4. React hook consuming the command.

### Skill: `write-evaluator-test`
When adding an owl-brain feature:
1. Write MockEngine scripted responses for the scenario.
2. Write mock tools with expected call sequences.
3. Assert evaluator score >= baseline.
4. Record baseline in owl-harness/baselines/.

### Skill: `externalize-config-prompts`
When asked to add/change a tunable value or an LLM prompt, follow WF-11 strictly:
1. Classify the change: CONFIG (URL, timeout, model id, numeric default, feature flag)
   or PROMPT (system/user/few-shot/extraction template).
2. CONFIG → goes into `src/config.rs` + `config/<crate>.toml` (secrets → env var only).
3. PROMPT → goes into `src/prompt.rs` (small) or `prompts/*.md` + `include_str!` (large).
4. Replace inline literals at call-sites with `config.<field>` or `prompt::<builder>(…)`.
5. Audit: grep for the old literal and for suspicious strings
   ("You are", "Schema:", ">= 40 chars multiline") — report any remaining hits.
6. Output: `[MOVED]` per literal relocated, `[VIOLATION]` per remaining inline value.

### Skill: `graph-locate`
When asked "where is X used?" / "what does this change affect?", follow WF-13:
1. Never grep first. Translate the question into SurrealQL using L1/L2 edges.
2. If the question is semantic ("similar to", "related to"), pair the graph query
   with a vector seed from L3.
3. Return a Vec<CodeNodeRef> with the graph path that justifies each hit.
4. If grep is unavoidable (e.g., a string constant), say so explicitly and log it
   as a distillation signal (cortex is missing a node kind).

### Skill: `run-in-sandbox`
When asked to execute, test, or verify agent-authored code, follow R-21:
1. Build a single `ExecutionPlan` (image, cmd, mounts, timeout, network=false).
2. Dispatch via `Sandbox::run(plan)` — never shell out directly.
3. Persist the `ExecutionOutcome` as a `test_run` row before returning.
4. On Failure: loop back to Plan with the stderr tail — never surface Failure as
   the final response.

### Skill: `reflect-and-distill`
At task end — or when asked to summarize a session — follow WF-14:
1. Write the `memory` row (deterministic, no LLM).
2. Run the distillation clustering query; if a new cluster crosses threshold, emit
   a new `insight` row and log its id.
3. Output: `[MEMORY <id>]` + zero or more `[INSIGHT <id> kind=… scope=…]`.

### Skill: `trace-data-flow`
When asked "how does X reach Y?", trace the data flow through the architecture:
```
User input → [app layer] → owl-brain (via trait call) →
  → owl-tower (LLM, via CompletionModel) OR
  → owl-armory (tool, via ToolExecutor) OR
  → owl-mcp (external, via McpClient)
  → observation → loop continues or returns
→ [app layer] formats response for user
```
Map the specific types at each boundary (all should be owl-protocol types).

---

## FORBIDDEN PATTERNS

```rust
// FORBIDDEN: unwrap in library code
let val = result.unwrap();

// FORBIDDEN: raw HTTP to LLM
let resp = client.post("https://api.anthropic.com/...").send().await?;

// FORBIDDEN: god function
async fn run_agent_and_save_memory_and_call_tools_and_log(...) { }

// FORBIDDEN: duplicate types across crates
// owl-brain/src/types.rs AND owl-armory/src/types.rs both define `ToolResult`

// FORBIDDEN: Mutex across await
let guard = mutex.lock().unwrap();
some_async_fn().await; // guard still held

// FORBIDDEN: commented-out code
// let old_impl = ...;

// FORBIDDEN: lateral crate import
// In owl-brain/Cargo.toml: owl-armory = { path = "..." } -- only owl-protocol allowed

// FORBIDDEN: direct concrete model construction in owl-brain
// owl-brain must never know about ClaudeAdapter or GeminiAdapter
use owl_tower::adapters::claude::ClaudeAdapter; // ← FORBIDDEN in owl-brain

// FORBIDDEN: business logic in Tauri command handler
#[tauri::command]
async fn chat(msg: String, state: State<AppState>) -> Result<String, String> {
    let tokens: Vec<_> = msg.split_whitespace().collect(); // ← business logic, FORBIDDEN
    let trimmed = tokens.join(" ");
    state.brain.run(&trimmed).await.map_err(|e| e.to_string())
    // Only the last two lines are acceptable; logic belongs in owl-brain
}

// FORBIDDEN: ad-hoc JSON across crate boundary
// Passing serde_json::Value between crates instead of owl-protocol types
async fn call_tool(&self, name: &str, args: serde_json::Value) -> serde_json::Value { }
//                                          ^^^^^^^^^^^^^^^^          ^^^^^^^^^^^^^^^^ FORBIDDEN
// Use: owl_protocol::ToolCall and owl_protocol::ToolResult instead

// FORBIDDEN: thiserror in app layer (owl-cli, owl-desktop)
// Use anyhow in apps, thiserror in crates
#[derive(thiserror::Error)] // ← FORBIDDEN in apps/owl-cli or apps/owl-desktop/src-tauri
enum CliError { }

// FORBIDDEN: concrete transport type in MCP client
// client.rs must only know McpTransport trait
use owl_mcp::transport::stdio::StdioTransport;
let t = StdioTransport::new(); // ← FORBIDDEN inside client.rs logic

// FORBIDDEN: inline prompt string in business code (R-19)
// Prompts must live in prompt.rs or prompts/*.md — never inline.
let agent = AgentBuilder::new(model)
    .preamble("You are an information-extraction engine. Return strict JSON...") // ← FORBIDDEN
    .build();

// FORBIDDEN: hardcoded tunable / endpoint / model id (R-19)
let store = SurrealStore::connect(SurrealConfig {
    endpoint:  "rocksdb:///var/lib/knight-owl/db".into(), // ← FORBIDDEN, must come from Config
    namespace: "knight_owl".into(),
    database:  "vault".into(),
}).await?;
let top_k: u64 = 8;          // ← FORBIDDEN magic number inside function body
let model_id = "claude-3-5-sonnet-20241022"; // ← FORBIDDEN, model id lives in config

// FORBIDDEN: std::env::var outside config.rs (R-19)
// owl-brain/src/reasoning_loop.rs
let key = std::env::var("ANTHROPIC_API_KEY").ok(); // ← FORBIDDEN here; only config.rs reads env

// FORBIDDEN: ad-hoc config parsing (R-19)
let cfg: serde_json::Value = serde_json::from_str(&raw)?; // ← FORBIDDEN; use a typed Config struct
let timeout = cfg["timeout"].as_u64().unwrap_or(30);
```

---

## REQUIRED PATTERNS

```rust
// REQUIRED: thiserror for library errors
#[derive(Debug, thiserror::Error)]
pub enum BrainError {
    #[error("memory store unavailable: {0}")]
    MemoryUnavailable(#[from] MemoryError),
}

// REQUIRED: trait injection
pub struct ReasoningLoop<M: rig::completion::CompletionModel> {
    model: M,
    memory: Arc<dyn MemoryStore>,
}

// REQUIRED: rig tool implementation
impl rig::tool::Tool for ReadFileTool {
    const NAME: &'static str = "read_file";
    type Error = ArmoryError;
    type Args = ReadFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> rig::tool::ToolDefinition { ... }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> { ... }
}

// REQUIRED: doc comment on every pub item
/// Executes one reasoning step: selects a tool, calls it, and returns the observation.
pub async fn step(&mut self, prompt: &str) -> Result<Observation, BrainError> { ... }

// REQUIRED: SAFETY comment on unsafe
unsafe {
    // SAFETY: pointer was obtained from Box::into_raw and ownership is transferred here.
    drop(Box::from_raw(ptr));
}

// REQUIRED: owl-brain wired via trait injection (not concrete types)
pub struct ReasoningLoop<M, T, S>
where
    M: rig::completion::CompletionModel,
    T: ToolExecutor,
    S: MemoryStore,
{
    model: Arc<M>,
    tools: Arc<T>,
    memory: Arc<S>,
    config: ReasoningConfig,
}

// REQUIRED: owl-protocol type crossing crate boundary
// owl-armory returns owl_protocol::ToolResult — not its own type
async fn call(&self, args: Self::Args) -> Result<owl_protocol::ToolResult, ArmoryError> { ... }

// REQUIRED: MCP client uses only McpTransport trait
pub struct McpClient {
    transport: Box<dyn McpTransport + Send + Sync>,
}
impl McpClient {
    pub fn new(transport: impl McpTransport + Send + Sync + 'static) -> Self {
        Self { transport: Box::new(transport) }
    }
}

// REQUIRED: thin Tauri command — max 10 lines, no logic
#[tauri::command]
async fn send_message(
    msg: owl_protocol::ipc::ChatInput,
    state: tauri::State<'_, AppState>,
) -> Result<owl_protocol::ipc::ChatOutput, String> {
    state.brain.run(msg).await.map_err(|e| e.to_string())
}

// REQUIRED: owl-protocol type with full derives
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ToolCall {
    pub name: String,
    pub args: serde_json::Value,
}

// REQUIRED: AgentState defined in owl-protocol, used in owl-brain
// owl-protocol::state
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AgentState { Idle, Planning, Acting, Observing }

// owl-brain::loop — transitions only, never redefines AgentState
use owl_protocol::state::AgentState;
fn transition(state: AgentState) -> AgentState {
    match state {
        AgentState::Idle      => AgentState::Planning,
        AgentState::Planning  => AgentState::Acting,
        AgentState::Acting    => AgentState::Observing,
        AgentState::Observing => AgentState::Idle,
    }
}

// REQUIRED: one Config struct per crate, loaded from file + env (R-19)
// crates/owl-vault/src/config.rs
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Config {
    pub endpoint:  String,
    pub namespace: String,
    pub database:  String,
    #[serde(default)]
    pub default_top_k: u64,
}

impl Config {
    /// Load defaults → config/owl-vault.toml → env overrides.
    pub fn load() -> Result<Self, VaultError> {
        let raw = std::fs::read_to_string("config/owl-vault.toml")
            .map_err(|e| VaultError::Config(e.to_string()))?;
        let mut cfg: Config = toml::from_str(&raw)
            .map_err(|e| VaultError::Config(e.to_string()))?;
        if let Ok(v) = std::env::var("OWL_VAULT_ENDPOINT") { cfg.endpoint = v; }
        Ok(cfg)
    }
}

// REQUIRED: prompts live in prompt.rs — constants only (R-19)
// crates/owl-cartographer/src/prompt.rs
pub const EXTRACTION_SYSTEM: &str = include_str!("../prompts/extraction_system.md");
pub fn extraction_user(chunk: &str) -> String {
    format!("Extract entities and relations from:\n\n---\n{chunk}\n---")
}

// REQUIRED: business code pulls prompts through prompt.rs — never inline
// crates/owl-cartographer/src/extract.rs
use crate::prompt::{extraction_user, EXTRACTION_SYSTEM};
let agent = AgentBuilder::new(model).preamble(EXTRACTION_SYSTEM).build();
let raw   = agent.prompt(extraction_user(chunk).as_str()).await?;

// REQUIRED: business code pulls tunables through Config — never magic numbers
// apps/owl-cli/src/commands/ask.rs
let chunks = cartographer.ask(&prompt, cfg.default_top_k, cfg.default_depth).await?;
```

---

## ROADMAP — Nervous-System Build-out

Four stages, each gated on the previous one passing WF-10 (architecture health).

### Stage 1 — Muscle (L1 Syntax)
- Stand up `owl-cortex` crate with `tree-sitter` + Rust grammar.
- Extend `HybridStore` with `file`, `code_node` tables + `DEFINES/CALLS/IMPORTS/CONTAINS` edges.
- CLI command: `owl index <path>` populates L1 for the current workspace.
- Exit criteria: re-indexing is idempotent; `owl-cortex` test fixtures round-trip.

### Stage 2 — Sense (L2 Logic + L4 Git)
- Add LSP client (`tower-lsp`) for cross-file REFERENCES/IMPLEMENTS/OVERRIDES.
- Ingest git history → `commit`, `pr` rows + RELATE to touched code_nodes.
- `HybridStore::impacted_nodes(file)` query lands; `owl-brain` Locate phase wired.
- Exit criteria: `graph-locate` skill can answer "who calls X?" without filesystem access.

### Stage 3 — Skill (owl-sandbox + Test Harness)
- Build `owl-sandbox` on top of `bollard`; `Sandbox` trait exposed via owl-protocol.
- Execute phase in owl-brain routes every edit through `Sandbox::run`.
- `test_run` rows persist to L4 with RELATE to the originating request.
- Exit criteria: R-21 enforced — no task completes without a `test_run` row.

### Stage 4 — Mind (Memory + Insights)
- Add `memory` + `insight` tables; implement WF-14 reflection + distillation job.
- `DEFINE EVENT` on memory insert rate triggers distillation.
- Plan phase consults active `insight` rows; Review phase consults `standard_node`.
- Exit criteria: repeat failures surface as `anti_pattern` insights within one session.

Each stage ships with: CLAUDE.md agent doc update, evaluator baseline in
`owl-harness/baselines/`, and an end-to-end integration test that walks the new layer.
