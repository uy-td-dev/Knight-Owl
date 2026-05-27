//! Hybrid graph + vector store contract.
//!
//! Unifies all access patterns for the 4-layer nervous system:
//! - **Vector** (L3): semantic search over embedded content
//! - **GraphRAG** (L3): entity/relation persistence + neighbour traversal
//! - **L1 Syntax**: file + code_node CRUD and structural edge queries
//!
//! Implemented by [`crate::surreal::SurrealStore`]; can be swapped for
//! in-memory or alternate backends without touching call sites (R-5).

use async_trait::async_trait;

use owl_protocol::code::{CodeEdge, CodeEdgeKind, CodeNode, FileNode, StandardNode, Violation};
use owl_protocol::experience::{Insight, TaskMemory, TestRun};
use owl_protocol::git::GitCommit;
use owl_protocol::graph::{Entity, Relation};
use owl_protocol::vector::{Embedding, VectorDocument, VectorMatch};

use crate::VaultError;

/// Unified graph + vector persistence layer.
#[async_trait]
pub trait HybridStore: Send + Sync {
    // ── L3 Vector ──────────────────────────────────────────────────────────

    /// Insert or upsert generic vector documents.
    async fn upsert_documents(&self, docs: Vec<VectorDocument>) -> Result<(), VaultError>;

    /// Nearest-neighbour semantic search.
    async fn vector_search(
        &self,
        query: Vec<f32>,
        limit: u64,
    ) -> Result<Vec<VectorMatch>, VaultError>;

    /// Delete documents by id.
    async fn delete_documents(&self, ids: Vec<String>) -> Result<(), VaultError>;

    // ── L3 GraphRAG ────────────────────────────────────────────────────────

    /// Upsert an entity node, optionally attaching an embedding for hybrid search.
    async fn upsert_entity(
        &self,
        entity: Entity,
        embedding: Option<Embedding>,
    ) -> Result<(), VaultError>;

    /// Upsert a directed relation edge. Endpoints must exist.
    async fn upsert_relation(&self, relation: Relation) -> Result<(), VaultError>;

    /// Fetch an entity by canonical id.
    async fn get_entity(&self, id: &str) -> Result<Option<Entity>, VaultError>;

    /// Return all entities reachable from `id` within `depth` hops.
    async fn neighbours(&self, id: &str, depth: usize) -> Result<Vec<Entity>, VaultError>;

    // ── L1 Syntax (file + code_node) ───────────────────────────────────────

    /// Upsert a tracked source file record.
    async fn upsert_file(&self, file: FileNode) -> Result<(), VaultError>;

    /// Upsert a parsed code element node.
    async fn upsert_code_node(&self, node: CodeNode) -> Result<(), VaultError>;

    /// Upsert a directed structural edge between two code elements.
    async fn upsert_code_edge(&self, edge: CodeEdge) -> Result<(), VaultError>;

    /// Return all `CodeNode`s contained in the given file path.
    async fn code_nodes_for_file(&self, path: &str) -> Result<Vec<CodeNode>, VaultError>;

    /// Return all outgoing edges of the given kind from a code_node.
    async fn code_edges_from(
        &self,
        from_id: &str,
        kind: CodeEdgeKind,
    ) -> Result<Vec<CodeNode>, VaultError>;

    /// Return all incoming edges of the given kind targeting a code_node.
    ///
    /// Used by WF-13 graph expansion to walk REFERENCES / CALLS edges
    /// backwards from a seed (e.g. "who calls `foo`?").
    /// Default impl returns an empty vec so older backends keep compiling.
    async fn code_edges_into(
        &self,
        _to_id: &str,
        _kind: CodeEdgeKind,
    ) -> Result<Vec<CodeNode>, VaultError> { Ok(Vec::new()) }

    /// Delete all code_nodes (and their edges) belonging to a file.
    ///
    /// Called before re-ingesting a changed file to avoid stale nodes.
    async fn delete_code_nodes_for_file(&self, path: &str) -> Result<(), VaultError>;

    /// Full-text search over `code_node.name` and `code_node.file_path`.
    ///
    /// Uses BM25 full-text index via SurrealDB `@@` operator (WF-13 BM25 seed).
    /// Falls back to substring match when the index is not available.
    async fn search_code_nodes(
        &self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<CodeNode>, VaultError>;

    /// Attach a precomputed embedding to an existing code_node.
    async fn update_code_node_embedding(
        &self,
        id: &str,
        embedding: Vec<f32>,
    ) -> Result<(), VaultError>;

    /// Semantic vector search over code_node embeddings.
    async fn vector_search_code_nodes(
        &self,
        query_embedding: Vec<f32>,
        limit: u64,
    ) -> Result<Vec<CodeNode>, VaultError>;

    /// BM25 keyword search — WF-13 `HybridStore::keyword_search` seed.
    ///
    /// Returns up to `limit` nodes ranked by BM25 relevance score.
    /// Used alongside `vector_search` for the hybrid retrieval pipeline.
    async fn keyword_search(
        &self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<CodeNode>, VaultError>;

    // ── L4 Git history ─────────────────────────────────────────────────────

    /// Persist a git commit in the L4 knowledge graph.
    async fn store_commit(&self, commit: GitCommit) -> Result<(), VaultError>;

    /// Return the most-recent `limit` commits, ordered newest-first.
    async fn recent_commits(&self, limit: u64) -> Result<Vec<GitCommit>, VaultError>;

    // ── L4 Experience ───────────────────────────────────────────────────────

    /// Persist a completed-task memory row (R-22 reflection step).
    async fn store_task_memory(&self, memory: TaskMemory) -> Result<(), VaultError>;

    /// Return insights whose scope starts with `scope_prefix`.
    ///
    /// Used during the Plan phase to inject constraints derived from past runs.
    async fn recall_insights(&self, scope_prefix: &str) -> Result<Vec<Insight>, VaultError>;

    /// Upsert a distilled insight row.
    ///
    /// Called only from the distillation job — never from business-logic code.
    async fn upsert_insight(&self, insight: Insight) -> Result<(), VaultError>;

    /// Fetch the most-recent task memories for distillation clustering.
    async fn recent_task_memories(&self, limit: u64) -> Result<Vec<TaskMemory>, VaultError>;

    /// Persist a sandbox verification run (R-21).
    ///
    /// Called by the reasoning loop after every `Sandbox::run` invocation.
    /// Default impl is a no-op so backends without L4 verification storage
    /// keep compiling.
    async fn store_test_run(&self, _run: TestRun) -> Result<(), VaultError> { Ok(()) }

    /// Fetch verification history for a given task, newest first.
    async fn test_runs_for_task(
        &self,
        _task_id: &str,
    ) -> Result<Vec<TestRun>, VaultError> { Ok(Vec::new()) }

    // ── R-22 Review phase ──────────────────────────────────────────────────

    /// Upsert a coding standard.  Idempotent.  Called at bootstrap to seed
    /// the rules from CLAUDE.md, and by callers wanting workspace-specific
    /// rules.  Default impl is a no-op.
    async fn upsert_standard(&self, _std: StandardNode) -> Result<(), VaultError> { Ok(()) }

    /// List every active standard the reviewer should check against.
    async fn list_standards(&self) -> Result<Vec<StandardNode>, VaultError> { Ok(Vec::new()) }

    /// Persist a violation discovered by the review phase.
    ///
    /// Also creates a `code_node ->violates-> standard_node` graph edge so
    /// the distillation job can walk violations alongside other code edges.
    async fn record_violation(&self, _v: Violation) -> Result<(), VaultError> { Ok(()) }

    /// Fetch every violation linked to a given task, newest first.
    async fn violations_for_task(
        &self,
        _task_id: &str,
    ) -> Result<Vec<Violation>, VaultError> { Ok(Vec::new()) }

    /// Wipe every row in the `insight` table.  Returns the number of
    /// rows deleted.  Default impl is a no-op for backends without a
    /// straightforward `DELETE` path.
    async fn clear_insights(&self) -> Result<u64, VaultError> { Ok(0) }

    /// Wipe every row in the `session_memory` table.  Useful when a
    /// small model gets stuck echoing past replies and the recall block
    /// dominates its context window.  Returns rows deleted.
    async fn clear_session_memory(&self) -> Result<u64, VaultError> { Ok(0) }

    // ── KB Graph queries ───────────────────────────────────────────────────

    /// Return aggregate counts for the KB overview.
    async fn kb_stats(&self) -> Result<KbStats, VaultError>;

    /// Return all tracked files with per-file node counts.
    async fn kb_files(&self) -> Result<Vec<FileWithStats>, VaultError>;

    /// Return all code_nodes for a file plus their inter-file edges.
    async fn kb_file_graph(&self, file_path: &str) -> Result<GraphData, VaultError>;

    /// Return all entities and their relations.
    async fn kb_entity_graph(&self) -> Result<EntityGraphData, VaultError>;
}

/// One hit in a graph BFS walk — carries the resolved node + its
/// shortest-path distance from any seed (0 = seed itself).
#[derive(Debug, Clone)]
pub struct WalkHit {
    pub node: CodeNode,
    /// Shortest-path distance from any seed, in edges (0 = seed).
    pub depth: usize,
}

/// Breadth-first walk over the code graph starting from `seed_ids`.
///
/// At each frontier hop the walk follows BOTH directions on every edge kind
/// in `kinds` — for `Calls`, this surfaces "what does X call" *and*
/// "who calls X".  The result is a `Vec<WalkHit>` containing every visited
/// node (including seeds), each annotated with its shortest distance to a
/// seed.  Result is capped at `max_nodes` to bound cost on dense graphs.
///
/// Used by WF-13 hybrid retrieval to expand the candidate set after the
/// vector / BM25 seeding step.
pub async fn walk_graph(
    store:     &dyn HybridStore,
    seed_ids:  Vec<String>,
    depth:     usize,
    kinds:     &[CodeEdgeKind],
    max_nodes: usize,
) -> Result<Vec<WalkHit>, VaultError> {
    use std::collections::{HashMap, HashSet};

    // Two separate sets — conflating them was the bug.
    //   `hits`     records every node we've discovered + its shallowest depth.
    //   `expanded` records nodes whose outgoing edges we've already followed
    //              (so we don't re-query the store for them).
    let mut hits:     HashMap<String, WalkHit> = HashMap::new();
    let mut expanded: HashSet<String>          = HashSet::new();
    // Seeds are tracked separately so they don't show up as their own
    // "neighbours" — graphs with back-edges (e.g. A calls B, B references A)
    // would otherwise re-emit the seed at depth 2.  Callers can merge their
    // seed payloads back in if they want.
    let seed_set:     HashSet<String>          = seed_ids.iter().cloned().collect();
    let mut frontier: Vec<String>              = seed_ids;

    for d in 0..=depth {
        let mut next_frontier: Vec<String> = Vec::new();
        for id in &frontier {
            if !expanded.insert(id.clone()) {
                continue; // already followed this node's edges
            }
            for kind in kinds {
                let out = store.code_edges_from(id, *kind).await?;
                let inc = store.code_edges_into(id, *kind).await?;
                for n in out.into_iter().chain(inc.into_iter()) {
                    if seed_set.contains(&n.id) {
                        continue; // never emit a seed as its own neighbour
                    }
                    let new_depth = d + 1;
                    let entry = hits.entry(n.id.clone()).or_insert_with(|| WalkHit {
                        node:  n.clone(),
                        depth: new_depth,
                    });
                    if entry.depth > new_depth {
                        entry.depth = new_depth;
                    }
                    if d < depth && !expanded.contains(&n.id) {
                        next_frontier.push(n.id);
                    }
                }
            }
            if hits.len() >= max_nodes {
                break;
            }
        }
        frontier = next_frontier;
        if frontier.is_empty() || hits.len() >= max_nodes {
            break;
        }
    }

    let mut out: Vec<WalkHit> = hits.into_values().collect();
    out.truncate(max_nodes);
    Ok(out)
}

// ── WF-13 hybrid retrieval ─────────────────────────────────────────────────

/// Tunables for [`hybrid_retrieve`].
#[derive(Debug, Clone)]
pub struct HybridRetrieveConfig {
    /// Final cap on snippets returned.
    pub top_k:        usize,
    /// Seeds requested from each source (BM25, vector).
    pub seed_k:       u64,
    /// Graph BFS depth in edges (1 = direct neighbours).
    pub depth:        usize,
    /// Hard cap on graph-expanded candidates before scoring.
    pub max_expanded: usize,
}

impl Default for HybridRetrieveConfig {
    fn default() -> Self {
        Self { top_k: 20, seed_k: 8, depth: 2, max_expanded: 50 }
    }
}

/// WF-13 hybrid retrieval: BM25 seed + vector seed + graph expansion → ranked
/// snippets.  Returns formatted `String`s ready to inject into an LLM prompt.
///
/// Vector seed is optional — pass `None` to run BM25-only (cheaper, weaker).
/// Used by both `owl-cli` and `owl-desktop` so both surfaces honour R-20.
pub async fn hybrid_retrieve(
    store:    &dyn HybridStore,
    embedder: Option<&dyn crate::embedder::Embedder>,
    prompt:   &str,
    config:   &HybridRetrieveConfig,
) -> Result<Vec<String>, VaultError> {
    use std::collections::HashMap;

    if prompt.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut scored: HashMap<String, ScoredNode> = HashMap::new();

    // BM25 seeds — fall back to substring search on backends without a
    // SEARCH index (kv-mem doesn't support BM25; it returns empty rather
    // than erroring so keyword_search's own fallback doesn't trigger).
    let mut bm25 = store.keyword_search(prompt, config.seed_k).await?;
    if bm25.is_empty() {
        bm25 = store.search_code_nodes(prompt, config.seed_k).await?;
    }
    for (rank, node) in bm25.into_iter().enumerate() {
        let bonus = rank_factor(rank, config.seed_k as usize);
        scored
            .entry(node.id.clone())
            .or_insert_with(|| ScoredNode::new(node.clone()))
            .add(0.30 * bonus, SeedSource::Bm25);
    }

    // Vector seeds (optional).
    if let Some(e) = embedder {
        if let Ok(emb) = e.embed(prompt).await {
            let vec_hits = store
                .vector_search_code_nodes(emb.values, config.seed_k)
                .await
                .unwrap_or_default();
            for (rank, node) in vec_hits.into_iter().enumerate() {
                let bonus = rank_factor(rank, config.seed_k as usize);
                scored
                    .entry(node.id.clone())
                    .or_insert_with(|| ScoredNode::new(node.clone()))
                    .add(0.40 * bonus, SeedSource::Vector);
            }
        }
    }

    // Graph expansion via Calls / References / Contains, both directions.
    let seed_ids: Vec<String> = scored.keys().cloned().collect();
    if !seed_ids.is_empty() {
        let kinds = [
            CodeEdgeKind::Calls,
            CodeEdgeKind::References,
            CodeEdgeKind::Contains,
        ];
        let hits = walk_graph(store, seed_ids, config.depth, &kinds, config.max_expanded).await?;
        for hit in hits {
            if scored.contains_key(&hit.node.id) {
                continue;
            }
            let depth_bonus = 0.30 / (hit.depth as f32 + 1.0);
            scored
                .entry(hit.node.id.clone())
                .or_insert_with(|| ScoredNode::new(hit.node.clone()))
                .add(depth_bonus, SeedSource::Graph(hit.depth));
        }
    }

    let mut ranked: Vec<ScoredNode> = scored.into_values().collect();
    ranked.sort_by(|a, b|
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
    );
    ranked.truncate(config.top_k);

    Ok(ranked.into_iter().map(format_scored_node).collect())
}

struct ScoredNode {
    node:    CodeNode,
    score:   f32,
    sources: Vec<SeedSource>,
}

impl ScoredNode {
    fn new(node: CodeNode) -> Self {
        Self { node, score: 0.0, sources: Vec::new() }
    }
    fn add(&mut self, delta: f32, src: SeedSource) {
        self.score += delta;
        self.sources.push(src);
    }
}

enum SeedSource { Bm25, Vector, Graph(usize) }

fn rank_factor(rank: usize, seed_k: usize) -> f32 {
    if seed_k == 0 { return 0.0; }
    (1.0 - rank as f32 / seed_k as f32).max(0.0)
}

fn format_scored_node(s: ScoredNode) -> String {
    let provenance = s.sources.iter().map(|src| match src {
        SeedSource::Bm25     => "bm25".to_string(),
        SeedSource::Vector   => "vector".to_string(),
        SeedSource::Graph(d) => format!("graph:{d}"),
    }).collect::<Vec<_>>().join(",");
    let mut out = format!(
        "[{}:L{}-{}] {:?} `{}`  ({}, score={:.2})",
        s.node.file_path, s.node.start_line, s.node.end_line,
        s.node.kind, s.node.name, provenance, s.score,
    );
    if !s.node.description.is_empty() {
        if let Some(line) = s.node.description.lines().next() {
            out.push_str(&format!("\n  doc: {line}"));
        }
    }
    if !s.node.preview.is_empty() {
        out.push_str(&format!("\n  {}", s.node.preview.trim()));
    }
    out
}

/// Aggregate counts for the KB overview.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KbStats {
    pub file_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    pub entity_count: u64,
    pub insight_count: u64,
}

/// File entry with node count for the file tree.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileWithStats {
    pub path: String,
    pub lang: String,
    pub node_count: u64,
}

/// Nodes + edges payload for the graph viewer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphData {
    pub nodes: Vec<CodeNode>,
    pub edges: Vec<CodeEdge>,
}

/// Entities + relations payload for the GraphRAG viewer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntityGraphData {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}
