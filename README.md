# Knight-Owl 🦉⚔️

> A modular, multi-LLM AI coding agent built in Rust — local-first, sandboxed, with a persistent knowledge graph and a friendly desktop UI.

Knight-Owl is a **full-stack agentic framework**: pluggable LLM providers
(Claude / Gemini / Ollama), native + MCP tools, a 4-layer knowledge graph
backed by SurrealDB, sandboxed execution, and a Tauri v2 desktop client
with a chibi owl companion.  Designed to feel like Claude Code or Cursor
but architected so every component — model, tool, memory store, sandbox —
is a trait object you can swap.

```
┌────────────────────────────────────────────────────────────────┐
│  React UI (Tauri)  ──►  Brain  ──►  Tools (native + MCP)        │
│        ▲                  │              │                      │
│        │                  ▼              ▼                      │
│   agent_event       SurrealDB        Sandbox (Docker)           │
│   pet_event       (L1-L4 graph)        (R-21)                   │
└────────────────────────────────────────────────────────────────┘
```

---

## Quick Start

### Prerequisites

- **Rust** 1.78+
- **Node.js** 20+  (for the desktop UI)
- **Docker**  (for SurrealDB; optional but recommended)
- **Tauri CLI**: `cargo install tauri-cli --version "^2.0"`

### Run in 3 commands

```bash
# 1. Start the knowledge-graph DB
docker compose up -d surrealdb

# 2. Configure: copy template, fill in keys
mkdir -p ~/.knight-owl
cp config/settings.example.json ~/.knight-owl/settings.json
# edit ~/.knight-owl/settings.json — set your API key OR keep `provider: ollama` for local

# 3. Launch
cd apps/owl-desktop/src-tauri
cargo tauri dev
```

First build takes ~5–10 minutes (Rust + Tauri = many crates).  Subsequent
runs are seconds.

### CLI alternative

```bash
cargo run -p owl-cli -- index .                            # build the code graph
cargo run -p owl-cli -- chat "explain how the loop works"  # one-shot question
cargo run -p owl-cli -- watch                              # hot-reindex on file changes
```

---

## What's in the box

### 🧠 Polyglot agent loop
- **Plan → Act → Observe** state machine (`owl-brain`)
- Parallel tool dispatch — agent issues multiple `read_file` calls at once
- Step-level + token-level streaming via Tauri events
- Sandboxed bash execution (Docker isolation, R-21)
- Per-tool approval policy (`bash`/`write_file`/`edit_file` ask first)
- Automatic prompt caching when on Anthropic (~10× cheaper cached tokens)
- Multi-modal: image attachments → native vision API (Claude / Gemini)

### 🗺 4-layer knowledge graph
All four layers live in the same SurrealDB; the agent walks them via
SurrealQL traversal — never grep / walkdir.

| Layer | What lives there | Built by |
|-------|------------------|----------|
| **L4** Experience | `commit`, `pr`, `test_run`, `memory`, `insight` | `owl-harness` runs feed it |
| **L3** Semantic   | `doc`, `code_chunk` (with embedding) | `owl-cartographer` + BM25 |
| **L2** Logic      | `symbol` (LSP-resolved) | `owl-cortex` LSP layer |
| **L1** Syntax     | `file`, `code_node` (Function / Class / Variable) | `owl-cortex` tree-sitter |

Plus a `standard_node` table holding SOLID / KISS / DRY / project rules
that nodes can `VIOLATES`-edge into during review.

### 🦉 Owl chibi companion
A draggable pet widget in the corner — reacts to your agent's activity.
Tool success bumps XP; tool failure dents happiness; idle decays needs.
Sprite renders from a codex-pets `.webp` atlas; pet evolves from Egg →
Chick → Owlet → Adult → Sage with `set_pet_level` dev cheat.

### 🔌 Extensibility today
- **MCP servers** — add tools via stdio / WS / HTTP, no recompile
- **Orchestra** — drop `*.yaml` agent / skill / workflow specs into
  `.knight-owl/`, watched + hot-reloaded
- **Hooks** (planned) — pre/post-tool shell callbacks
- **WASM plugins** (planned) — language-agnostic tool plugins via
  Wasmtime Component Model

### 💾 Sessions persist
- Conversations in `~/.knight-owl/chats/<workspace_id>/<conv_id>.json`
- Agent-event log per conv → replay full reasoning trace on resume
  (`load_event_log` Tauri command)
- Memory + insights survive app restarts via SurrealDB
- Pet level + stats persisted too — close app, owlet's still hungry tomorrow

---

## Workspace layout

```
knight-owl/
├── apps/
│   ├── owl-cli/               # scriptable CLI (clap)
│   └── owl-desktop/
│       ├── src-tauri/         # Rust backend, Tauri commands
│       └── ui/                # React + Vite + Tauri v2 frontend
│
├── shared/
│   └── owl-protocol/          # Shared types (zero internal deps)
│
├── crates/
│   ├── owl-brain/             # Orchestrator: reasoning loop, memory, factory
│   ├── owl-tower/             # LLM adapters: Claude / Gemini / Ollama, cache
│   ├── owl-armory/            # Native tools: read/write/edit/grep/bash/...
│   ├── owl-mcp/               # MCP client — stdio / WS / HTTP transports
│   ├── owl-vault/             # SurrealDB hybrid graph + vector + pet store
│   ├── owl-cortex/            # Tree-sitter + LSP → L1/L2 layers
│   ├── owl-cartographer/      # GraphRAG entity/relation extraction → L3
│   ├── owl-sandbox/           # Docker (bollard) + local subprocess runner
│   ├── owl-orchestra/         # Agents / skills / workflows registry + watcher
│   └── owl-harness/           # Mock engine + evaluator (dev-only)
│
├── config/
│   ├── settings.example.json  # ← copy to ~/.knight-owl/settings.json
│   ├── owl-tower.toml         # provider / model defaults
│   ├── owl-vault.toml         # SurrealDB defaults
│   ├── owl-brain.toml         # max_steps / memory_context_limit
│   └── owl-cortex.toml        # tree-sitter language toggles
│
├── docker-compose.yml         # SurrealDB + optional Ollama + CLI service
├── Dockerfile.cli             # containerised owl-cli
└── CLAUDE.md                  # agent-development guide (architecture rules)
```

---

## Configuration

All runtime config lives in **one file**: `~/.knight-owl/settings.json`.
Copy from `config/settings.example.json` and edit:

```json
{
  "provider":     "ollama",
  "model":        "qwen2.5-coder:7b",
  "cache_policy": "ephemeral",

  "anthropic_api_key": null,
  "gemini_api_key":    null,

  "ollama_base_url":    "http://localhost:11434",
  "ollama_embed_model": "nomic-embed-text",

  "surreal_endpoint":  "ws://localhost:8000",
  "surreal_namespace": "knight_owl",
  "surreal_database":  "vault",
  "surreal_username":  "root",
  "surreal_password":  "root",

  "brain_max_steps":            10,
  "brain_memory_context_limit": 15,
  "sandbox_bash_enabled":       true
}
```

**Priority** (highest wins): shell env var → `settings.json` → crate
default.  This means CI / one-off overrides like
`ANTHROPIC_API_KEY=… cargo tauri dev` always trump the file.

---

## Providers

| Provider  | Setup |
|-----------|-------|
| **Anthropic** (Claude) | Set `anthropic_api_key`.  Vision + prompt cache active.  Best agent behaviour. |
| **Google** (Gemini)    | Set `gemini_api_key`.  Also drives the default embedder (3072-dim). |
| **Ollama** (local)     | `ollama serve` + `ollama pull qwen2.5-coder:7b`.  Set `provider: ollama`.  Note: tool-use reliability scales with model size — `gemma:e2b` and `phi-3-mini` aren't enough; aim for 7B+. |

Embeddings: Gemini if its key is present, otherwise local Ollama
(`nomic-embed-text` / `all-minilm`), otherwise a hash-based fallback
(degraded semantic recall — install one of the above for real RAG).

---

## Knowledge base — index your project

```bash
# After SurrealDB is up + workspace picked:
export OWL_VAULT_USER=root OWL_VAULT_PASS=root
export OWL_WORKSPACE=/path/to/your/project
cargo run -p owl-cli -- index .
```

Or from the desktop app: pet menu / sidebar `/index` slash command.

What it builds:
1. Tree-sitter parses every `.rs`/`.ts`/`.py`/... → L1 `code_node` rows
2. LSP resolves cross-file references → L2 `symbol` edges
3. Each node's docstring is embedded → L3 vectors + BM25 index
4. (Optional) cartographer extracts entities from docs → L3 graph

Now agent's `<code_context>` block ships actual code snippets retrieved
via hybrid (vector + graph-walk) search per the WF-13 pipeline.

---

## Docker stack

```bash
# Minimal — just the DB
docker compose up -d surrealdb

# + local LLM
docker compose --profile ollama up -d
docker compose exec ollama ollama pull qwen2.5-coder:7b

# Headless CLI in a container
docker compose --profile cli run --rm owl-cli chat "what does owl-brain do"
```

Desktop GUI is NOT containerised — Tauri needs a host window manager.
Run it from your shell with `cargo tauri dev` while the stack is up.

---

## Architecture rules

The codebase enforces a strict dependency graph (see `CLAUDE.md` for the
full R-13 spec):

```
owl-protocol  →  (zero internal deps)
owl-brain     →  owl-protocol, rig-core              (never tower/armory/mcp)
owl-tower     →  owl-protocol, rig-core
owl-armory    →  owl-protocol, rig-core
owl-mcp       →  owl-protocol
owl-vault     →  owl-protocol, rig-core, surrealdb
owl-cortex    →  owl-protocol, owl-vault, tree-sitter, tower-lsp
owl-sandbox   →  owl-protocol, bollard
owl-harness   →  every crate above       [dev-dependency only]
owl-cli       →  owl-brain, owl-protocol, anyhow
owl-desktop   →  owl-brain, owl-protocol, anyhow
```

`owl-brain` never imports `owl-tower` / `owl-armory` directly — every
dependency arrives as `Arc<dyn Trait>` injected at startup.  This is the
testability backbone: the brain compiles against mock impls in
`owl-harness` and you can swap any component without touching its
consumers.

---

## Status / Roadmap

✅ **Done**
- Multi-provider tower (Claude / Gemini / Ollama) with native streaming
- Parallel tool dispatch + per-tool approval policy
- Anthropic prompt caching (auto)
- Vision input (image attachments → native multi-modal request)
- Sandboxed bash via Docker
- SurrealDB-backed memory + insights + 4-layer code graph
- GraphRAG retrieval (BM25 + vector + graph walk)
- Orchestra registry (agents/skills/workflows) with hot-reload
- Owl chibi pet with codex-pets sprite + auto-think mind task
- Tauri commands: `clear_memory`, `clear_insights`, `set_pet_level`,
  `resolve_tool_approval`, `load_event_log`, etc.
- Unified `settings.json` (no scattered env vars required)

🚧 **In progress / planned**
- Native function-calling protocol (replace embedded-JSON tool format
  for more reliable local-model tool use)
- WASM plugin host (Wasmtime Component Model)
- IDE-mode LSP server exposing the knowledge graph
- Conversation branching UI
- Cost router (cheap model for retrieval, smart model for execution)

---

## Development

```bash
# Build everything
cargo check --workspace

# Run tests
cargo test --workspace

# Brain tests in isolation
cargo test -p owl-brain --lib

# Run UI typecheck
cd apps/owl-desktop/ui && npx tsc --noEmit

# Build production desktop bundle (macOS / Linux / Windows)
cd apps/owl-desktop/src-tauri && cargo tauri build
```

See `CLAUDE.md` for the in-depth contributor guide — agent
responsibilities per crate, the SOLID/KISS/DRY rules, configuration
externalization patterns (R-19), and the workflow for adding a new tool
(WF-1), provider (WF-2), MCP transport (WF-6), Tauri command (WF-8), etc.

---

## License

MIT — see [LICENSE](LICENSE).
