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

use owl_protocol::code::{CodeEdge, CodeEdgeKind, CodeNode, FileNode};
use owl_protocol::experience::{Insight, TaskMemory};
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
