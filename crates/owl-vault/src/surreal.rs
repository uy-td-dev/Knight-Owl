//! SurrealDB-backed hybrid graph + vector store.
//!
//! Schema (all tables defined in `connect`):
//! - L3 GraphRAG: `entity`, `doc`, `rel` (RELATION FROM entity TO entity)
//! - L1 Syntax:   `file`, `code_node`, and relation tables
//!                `defines` (file→code_node), `contains`, `calls`, `imports`,
//!                `references`, `implements`, `overrides` (all code_node→code_node)

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};
use surrealdb::engine::any::{connect, Any};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;
use tracing::{debug, info};

use owl_protocol::code::{
    CodeEdge, CodeEdgeKind, CodeNode, CodeNodeKind, FileNode,
    Severity, StandardKind, StandardNode, Violation,
};
use owl_protocol::experience::{Insight, InsightKind, TaskMemory, TaskOutcome, TestRun};
use owl_protocol::sandbox::ExecutionOutcome;
use owl_protocol::git::GitCommit;
use owl_protocol::graph::{Entity, Relation};
use owl_protocol::vector::{Embedding, VectorDocument, VectorMatch};

use crate::store::HybridStore;
use crate::VaultError;

/// Configuration for [`SurrealStore`].
#[derive(Debug, Clone)]
pub struct SurrealConfig {
    /// SurrealDB endpoint. Examples:
    /// - `"mem://"` (in-process, ephemeral)
    /// - `"rocksdb:///var/lib/knight-owl/db"` (embedded persistent)
    /// - `"ws://localhost:8000"` (remote server)
    pub endpoint: String,
    /// Namespace.
    pub namespace: String,
    /// Database name.
    pub database: String,
    /// Optional root username for remote servers that require auth.
    pub username: Option<String>,
    /// Optional root password.
    pub password: Option<String>,
}

impl SurrealConfig {
    /// In-memory ephemeral config — useful for tests.
    pub fn memory() -> Self {
        Self {
            endpoint: "mem://".into(),
            namespace: "knight_owl".into(),
            database: "vault".into(),
            username: None,
            password: None,
        }
    }

    /// Embedded persistent config rooted at `path`.
    pub fn rocksdb(path: impl Into<String>) -> Self {
        Self {
            endpoint: format!("rocksdb://{}", path.into()),
            namespace: "knight_owl".into(),
            database: "vault".into(),
            username: None,
            password: None,
        }
    }
}

/// SurrealDB-backed hybrid store.
pub struct SurrealStore {
    db: Surreal<Any>,
}

impl SurrealStore {
    /// Borrow the underlying `Surreal<Any>` handle.
    ///
    /// Used by satellite stores in this crate (e.g.
    /// [`crate::SurrealSchedulerStore`]) that need to share the same
    /// namespace + db selection without re-connecting.
    pub fn db(&self) -> &Surreal<Any> { &self.db }

    /// Connect, select the namespace/database, and ensure the schema is ready.
    pub async fn connect(cfg: SurrealConfig) -> Result<Self, VaultError> {
        info!(endpoint = %cfg.endpoint, ns = %cfg.namespace, db = %cfg.database, "connecting to surrealdb");
        let db = connect(&cfg.endpoint).await?;

        // Sign in as root if credentials supplied (typical for ws://… servers).
        if let (Some(u), Some(p)) = (cfg.username.as_ref(), cfg.password.as_ref()) {
            info!(user = %u, "signing into surrealdb as root");
            db.signin(Root { username: u.clone(), password: p.clone() })
                .await
                .map_err(|e| VaultError::Surreal(format!("signin failed: {e}")))?;
        }

        db.use_ns(&cfg.namespace).use_db(&cfg.database).await?;
        info!("surrealdb namespace+database selected");

        db.query(
            // L3 — GraphRAG
            "DEFINE TABLE IF NOT EXISTS entity   SCHEMALESS; \
             DEFINE TABLE IF NOT EXISTS doc      SCHEMALESS; \
             DEFINE TABLE IF NOT EXISTS rel      SCHEMALESS TYPE RELATION FROM entity TO entity; \
             \
             DEFINE TABLE IF NOT EXISTS file      SCHEMALESS; \
             DEFINE TABLE IF NOT EXISTS code_node SCHEMALESS; \
             \
             DEFINE TABLE IF NOT EXISTS defines     SCHEMALESS TYPE RELATION FROM file      TO code_node; \
             DEFINE TABLE IF NOT EXISTS contains    SCHEMALESS TYPE RELATION FROM code_node TO code_node; \
             DEFINE TABLE IF NOT EXISTS calls       SCHEMALESS TYPE RELATION FROM code_node TO code_node; \
             DEFINE TABLE IF NOT EXISTS imports     SCHEMALESS TYPE RELATION FROM code_node TO code_node; \
             DEFINE TABLE IF NOT EXISTS references  SCHEMALESS TYPE RELATION FROM code_node TO code_node; \
             DEFINE TABLE IF NOT EXISTS implements  SCHEMALESS TYPE RELATION FROM code_node TO code_node; \
             DEFINE TABLE IF NOT EXISTS overrides   SCHEMALESS TYPE RELATION FROM code_node TO code_node; \
             \
             DEFINE TABLE IF NOT EXISTS session_memory SCHEMALESS; \
             \
             DEFINE TABLE IF NOT EXISTS task_memory SCHEMALESS; \
             DEFINE TABLE IF NOT EXISTS insight      SCHEMALESS; \
             DEFINE TABLE IF NOT EXISTS test_run     SCHEMALESS; \
             \
             DEFINE TABLE IF NOT EXISTS commit SCHEMALESS; \
             DEFINE TABLE IF NOT EXISTS pr     SCHEMALESS; \
             \
             DEFINE TABLE IF NOT EXISTS standard_node SCHEMALESS; \
             DEFINE TABLE IF NOT EXISTS violation     SCHEMALESS; \
             DEFINE TABLE IF NOT EXISTS violates  SCHEMALESS \
                 TYPE RELATION FROM code_node TO standard_node; \
             \
             DEFINE TABLE IF NOT EXISTS cron_task SCHEMALESS;",
        )
        .await?
        .check()?;

        // BM25 full-text search indexes — only supported by the rocksdb backend.
        // Ignored silently on kv-mem (used in tests and lightweight deployments).
        db.query(
            "DEFINE ANALYZER IF NOT EXISTS code_analyzer \
               TOKENIZERS blank,class FILTERS lowercase; \
             DEFINE INDEX IF NOT EXISTS code_node_name_ft \
               ON code_node FIELDS name \
               SEARCH ANALYZER code_analyzer BM25; \
             DEFINE INDEX IF NOT EXISTS code_node_file_ft \
               ON code_node FIELDS file_path \
               SEARCH ANALYZER code_analyzer BM25; \
             DEFINE ANALYZER IF NOT EXISTS conv_analyzer \
               TOKENIZERS blank,class FILTERS lowercase; \
             DEFINE INDEX IF NOT EXISTS session_memory_content_ft \
               ON session_memory FIELDS content \
               SEARCH ANALYZER conv_analyzer BM25;",
        )
        .await
        .ok(); // Non-fatal — kv-mem does not support SEARCH indexes.

        let store = Self { db };
        // Seed default coding standards from CLAUDE.md (R-22 Review phase).
        store.seed_default_standards().await?;
        Ok(store)
    }

    /// Seed the rules from CLAUDE.md into `standard_node` on every connect.
    /// UPSERT semantics make this idempotent — re-running with a new rule
    /// set adds/updates entries without duplicating them.
    async fn seed_default_standards(&self) -> Result<(), VaultError> {
        use StandardKind::*;
        use Severity::*;
        let defaults = [
            ("R-1-function-length", "Function ≤ 30 lines", Solid, Block,
             "R-1 (Single Responsibility): split any function longer than 30 source lines."),
            ("R-1-single-word-name", "Function name does one thing", Solid, Warn,
             "R-1: functions with 'and'/'or'/'also' in the name do too many things — split them."),
            ("R-9-no-unwrap", "No unwrap/expect in library code", Project, Block,
             "R-9: never call .unwrap() or .expect() in library crates — return Result instead."),
            ("R-10-doc-pub", "Every pub item has a doc comment", Project, Warn,
             "R-10: pub fn / pub struct / pub trait require a /// doc comment explaining intent."),
        ];
        for (id, name, kind, sev, rule) in defaults {
            let std = StandardNode {
                id:        id.into(),
                name:      name.into(),
                kind,
                severity:  sev,
                rule_text: rule.into(),
            };
            // Use HybridStore impl directly to reuse the upsert path.
            HybridStore::upsert_standard(self, std).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl HybridStore for SurrealStore {
    async fn upsert_documents(&self, docs: Vec<VectorDocument>) -> Result<(), VaultError> {
        for doc in docs {
            let embedding = doc
                .embedding
                .ok_or_else(|| VaultError::Embedding(format!("missing embedding for {}", doc.id)))?;
            let metadata: Value = serde_json::to_value(&doc.metadata)?;
            self.db
                .query(
                    "UPSERT type::record('doc', $id) CONTENT { \
                       content: $content, embedding: $embedding, metadata: $metadata \
                     }",
                )
                .bind(json!({
                    "id": doc.id,
                    "content": doc.content,
                    "embedding": embedding.values,
                    "metadata": metadata,
                }))
                .await?
                .check()?;
        }
        Ok(())
    }

    async fn vector_search(
        &self,
        query: Vec<f32>,
        limit: u64,
    ) -> Result<Vec<VectorMatch>, VaultError> {
        let sql = "SELECT meta::id(id) AS id, content, embedding, metadata, \
                   vector::similarity::cosine(embedding, $q) AS score \
                   FROM doc WHERE embedding != NONE \
                   ORDER BY score DESC LIMIT $k";
        let mut resp = self
            .db
            .query(sql)
            .bind(json!({ "q": query, "k": limit as i64 }))
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_vector_match).collect())
    }

    async fn delete_documents(&self, ids: Vec<String>) -> Result<(), VaultError> {
        for id in ids {
            self.db
                .query("DELETE type::record('doc', $id)")
                .bind(json!({ "id": id }))
                .await?
                .check()?;
        }
        Ok(())
    }

    async fn upsert_entity(
        &self,
        entity: Entity,
        embedding: Option<Embedding>,
    ) -> Result<(), VaultError> {
        let embedding_values: Option<Vec<f32>> = embedding.map(|e| e.values);
        self.db
            .query(
                "UPSERT type::record('entity', $id) CONTENT { \
                   name: $name, kind: $kind, description: $description, embedding: $embedding \
                 }",
            )
            .bind(json!({
                "id": entity.id,
                "name": entity.name,
                "kind": entity.kind,
                "description": entity.description,
                "embedding": embedding_values,
            }))
            .await?
            .check()?;
        Ok(())
    }

    async fn upsert_relation(&self, relation: Relation) -> Result<(), VaultError> {
        self.db
            .query(
                "LET $src_rec = type::record('entity', $src); \
                 LET $tgt_rec = type::record('entity', $tgt); \
                 RELATE $src_rec -> rel -> $tgt_rec \
                   SET label = $label, weight = $weight",
            )
            .bind(json!({
                "src": relation.source,
                "tgt": relation.target,
                "label": relation.label,
                "weight": relation.weight,
            }))
            .await?
            .check()?;
        Ok(())
    }

    async fn get_entity(&self, id: &str) -> Result<Option<Entity>, VaultError> {
        let mut resp = self
            .db
            .query(
                "SELECT meta::id(id) AS id, name, kind, description \
                   FROM type::record('entity', $id)",
            )
            .bind(json!({ "id": id }))
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().next().and_then(row_to_entity))
    }

    async fn neighbours(&self, id: &str, depth: usize) -> Result<Vec<Entity>, VaultError> {
        if depth == 0 {
            return Ok(Vec::new());
        }
        let mut seen: HashMap<String, Entity> = HashMap::new();
        let mut frontier: Vec<String> = vec![id.to_string()];

        for _ in 0..depth {
            if frontier.is_empty() {
                break;
            }
            let sql = "SELECT meta::id(id) AS id, name, kind, description \
                       FROM (SELECT VALUE ->rel->entity FROM type::record('entity', $start)) \
                       FETCH id";
            let mut next: Vec<String> = Vec::new();
            for start in frontier.drain(..) {
                let mut resp = self
                    .db
                    .query(sql)
                    .bind(json!({ "start": start }))
                    .await?
                    .check()?;
                let rows: Vec<Value> = resp.take(0)?;
                for row in rows {
                    if let Some(entity) = row_to_entity(row) {
                        if !seen.contains_key(&entity.id) && entity.id != id {
                            next.push(entity.id.clone());
                            seen.insert(entity.id.clone(), entity);
                        }
                    }
                }
            }
            frontier = next;
        }
        Ok(seen.into_values().collect())
    }

    // ── L1 Syntax ─────────────────────────────────────────────────────────

    async fn upsert_file(&self, file: FileNode) -> Result<(), VaultError> {
        self.db
            .query(
                "UPSERT type::record('file', $id) CONTENT { \
                   path: $path, lang: $lang, content_hash: $hash \
                 }",
            )
            .bind(json!({
                "id":   sanitize_id(&file.path),
                "path": file.path,
                "lang": file.lang,
                "hash": file.content_hash,
            }))
            .await?
            .check()?;
        Ok(())
    }

    async fn upsert_code_node(&self, node: CodeNode) -> Result<(), VaultError> {
        let kind = serde_json::to_value(&node.kind)?;
        self.db
            .query(
                "UPSERT type::record('code_node', $id) CONTENT { \
                   file_path: $file_path, name: $name, kind: $kind, \
                   start_line: $start_line, end_line: $end_line, preview: $preview, \
                   visibility: $visibility, qualifiers: $qualifiers, description: $description \
                 }",
            )
            .bind(json!({
                "id":          node.id,
                "file_path":   node.file_path,
                "name":        node.name,
                "kind":        kind,
                "start_line":  node.start_line,
                "end_line":    node.end_line,
                "preview":     node.preview,
                "visibility":  node.visibility,
                "qualifiers":  node.qualifiers,
                "description": node.description,
            }))
            .await?
            .check()?;
        Ok(())
    }

    async fn upsert_code_edge(&self, edge: CodeEdge) -> Result<(), VaultError> {
        let table = edge_table(&edge.kind);
        let (src_table, tgt_table) = edge_endpoints(&edge.kind);
        // SurrealDB v3: type::record() cannot appear directly in RELATE;
        // assign to LET variables first.
        self.db
            .query(format!(
                "LET $src = type::record('{src_table}', $from); \
                 LET $tgt = type::record('{tgt_table}', $to); \
                 RELATE $src -> {table} -> $tgt"
            ))
            .bind(json!({ "from": edge.from, "to": edge.to }))
            .await?
            .check()?;
        Ok(())
    }

    async fn code_nodes_for_file(&self, path: &str) -> Result<Vec<CodeNode>, VaultError> {
        let mut resp = self
            .db
            .query(
                "SELECT meta::id(id) AS id, file_path, name, kind, \
                        start_line, end_line, preview, \
                        visibility, qualifiers, description \
                 FROM code_node WHERE file_path = $path",
            )
            .bind(json!({ "path": path }))
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_code_node).collect())
    }

    async fn code_edges_from(
        &self,
        from_id: &str,
        kind: CodeEdgeKind,
    ) -> Result<Vec<CodeNode>, VaultError> {
        let table = edge_table(&kind);
        let sql = format!(
            "SELECT meta::id(id) AS id, file_path, name, kind, \
                    start_line, end_line, preview \
             FROM (SELECT ->{table}->code_node AS nodes \
                   FROM type::record('code_node', $id))[0].nodes.*"
        );
        let mut resp =
            self.db.query(sql).bind(json!({ "id": from_id })).await?.check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_code_node).collect())
    }

    async fn code_edges_into(
        &self,
        to_id: &str,
        kind: CodeEdgeKind,
    ) -> Result<Vec<CodeNode>, VaultError> {
        let table = edge_table(&kind);
        let sql = format!(
            "SELECT meta::id(id) AS id, file_path, name, kind, \
                    start_line, end_line, preview \
             FROM (SELECT <-{table}<-code_node AS nodes \
                   FROM type::record('code_node', $id))[0].nodes.*"
        );
        let mut resp =
            self.db.query(sql).bind(json!({ "id": to_id })).await?.check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_code_node).collect())
    }

    async fn delete_code_nodes_for_file(&self, path: &str) -> Result<(), VaultError> {
        self.db
            .query("DELETE code_node WHERE file_path = $path")
            .bind(json!({ "path": path }))
            .await?
            .check()?;
        Ok(())
    }

    async fn search_code_nodes(
        &self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<CodeNode>, VaultError> {
        // Try BM25 full-text search first (@@ operator). On kv-mem backends that
        // don't support SEARCH indexes, fall back to substring match.
        let bm25_sql =
            "SELECT meta::id(id) AS id, file_path, name, kind, \
                    start_line, end_line, preview \
             FROM code_node \
             WHERE name @@ $q OR file_path @@ $q \
             LIMIT $limit";
        let fallback_sql =
            "SELECT meta::id(id) AS id, file_path, name, kind, \
                    start_line, end_line, preview \
             FROM code_node \
             WHERE string::contains(string::lowercase(name), $q) \
                OR string::contains(string::lowercase(file_path), $q) \
             LIMIT $limit";

        let q_lower = query.to_lowercase();
        let rows: Vec<serde_json::Value> = if let Ok(r) = self
            .db
            .query(bm25_sql)
            .bind(json!({ "q": query, "limit": limit as i64 }))
            .await
            .and_then(|mut r| { let v: Result<Vec<_>, _> = r.take(0); v.map_err(Into::into) })
        {
            r
        } else {
            let mut resp = self
                .db
                .query(fallback_sql)
                .bind(json!({ "q": q_lower, "limit": limit as i64 }))
                .await?
                .check()?;
            resp.take(0)?
        };
        Ok(rows.into_iter().filter_map(row_to_code_node).collect())
    }

    async fn keyword_search(
        &self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<CodeNode>, VaultError> {
        // BM25-ranked search on rocksdb. Falls back to search_code_nodes on kv-mem.
        let bm25_sql =
            "SELECT meta::id(id) AS id, file_path, name, kind, \
                    start_line, end_line, preview, \
                    visibility, qualifiers, description, \
                    search::score(1) AS _score \
             FROM code_node \
             WHERE name @1@ $q OR file_path @2@ $q \
             ORDER BY _score DESC \
             LIMIT $limit";

        let rows: Vec<serde_json::Value> = if let Ok(r) = self
            .db
            .query(bm25_sql)
            .bind(json!({ "q": query, "limit": limit as i64 }))
            .await
            .and_then(|mut r| { let v: Result<Vec<_>, _> = r.take(0); v.map_err(Into::into) })
        {
            r
        } else {
            return self.search_code_nodes(query, limit).await;
        };
        Ok(rows.into_iter().filter_map(row_to_code_node).collect())
    }

    async fn update_code_node_embedding(
        &self,
        id: &str,
        embedding: Vec<f32>,
    ) -> Result<(), VaultError> {
        self.db
            .query("UPDATE type::record('code_node', $id) SET embedding = $emb")
            .bind(json!({ "id": id, "emb": embedding }))
            .await?
            .check()?;
        Ok(())
    }

    async fn vector_search_code_nodes(
        &self,
        query_embedding: Vec<f32>,
        limit: u64,
    ) -> Result<Vec<CodeNode>, VaultError> {
        let sql = "SELECT meta::id(id) AS id, file_path, name, kind, \
                          start_line, end_line, preview, \
                          visibility, qualifiers, description, \
                          vector::similarity::cosine(embedding, $q) AS score \
                   FROM code_node WHERE embedding != NONE \
                   ORDER BY score DESC LIMIT $k";
        let mut resp = self
            .db
            .query(sql)
            .bind(json!({ "q": query_embedding, "k": limit as i64 }))
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_code_node).collect())
    }

    // ── L4 Git history ─────────────────────────────────────────────────────

    async fn store_commit(&self, commit: GitCommit) -> Result<(), VaultError> {
        let files = serde_json::to_value(&commit.files_changed)?;
        self.db
            .query(
                "UPSERT type::record('commit', $hash) CONTENT { \
                   hash: $hash, short_hash: $short, message: $msg, \
                   author_name: $author, author_email: $email, \
                   timestamp: $ts, files_changed: $files, \
                   insertions: $ins, deletions: $del \
                 }",
            )
            .bind(json!({
                "hash":  commit.hash,
                "short": commit.short_hash,
                "msg":   commit.message,
                "author": commit.author_name,
                "email": commit.author_email,
                "ts":    commit.timestamp,
                "files": files,
                "ins":   commit.insertions,
                "del":   commit.deletions,
            }))
            .await?
            .check()?;
        Ok(())
    }

    async fn recent_commits(&self, limit: u64) -> Result<Vec<GitCommit>, VaultError> {
        let mut resp = self
            .db
            .query(
                "SELECT hash, short_hash, message, author_name, author_email, \
                        timestamp, files_changed, insertions, deletions \
                 FROM commit ORDER BY timestamp DESC LIMIT $limit",
            )
            .bind(json!({ "limit": limit as i64 }))
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_commit).collect())
    }

    // ── L4 Experience ───────────────────────────────────────────────────────

    async fn store_task_memory(&self, memory: TaskMemory) -> Result<(), VaultError> {
        let actions = serde_json::to_value(&memory.actions)?;
        let outcome = serde_json::to_value(&memory.outcome)?;
        let code_refs = serde_json::to_value(&memory.code_refs)?;
        self.db
            .query(
                "UPSERT type::record('task_memory', $id) CONTENT { \
                   id: $id, request: $req, actions: $actions, outcome: $outcome, \
                   code_refs: $code_refs, session_id: $sid, \
                   input_tokens: $in_tok, output_tokens: $out_tok, \
                   created: time::now() \
                 }",
            )
            .bind(json!({
                "id":        memory.id,
                "req":       memory.request,
                "actions":   actions,
                "outcome":   outcome,
                "code_refs": code_refs,
                "sid":       memory.session_id,
                "in_tok":    memory.input_tokens,
                "out_tok":   memory.output_tokens,
            }))
            .await?
            .check()?;
        Ok(())
    }

    async fn recall_insights(&self, scope_prefix: &str) -> Result<Vec<Insight>, VaultError> {
        let mut resp = self
            .db
            .query(
                "SELECT meta::id(id) AS id, kind, scope, summary, evidence, created \
                 FROM insight \
                 WHERE string::starts_with(scope, $prefix) \
                 ORDER BY created DESC \
                 LIMIT 20",
            )
            .bind(json!({ "prefix": scope_prefix }))
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_insight).collect())
    }

    async fn upsert_insight(&self, insight: Insight) -> Result<(), VaultError> {
        let kind = serde_json::to_value(&insight.kind)?;
        let evidence = serde_json::to_value(&insight.evidence)?;
        self.db
            .query(
                "UPSERT type::record('insight', $id) CONTENT { \
                   id: $id, kind: $kind, scope: $scope, summary: $summary, \
                   evidence: $evidence, created: time::now() \
                 }",
            )
            .bind(json!({
                "id":       insight.id,
                "kind":     kind,
                "scope":    insight.scope,
                "summary":  insight.summary,
                "evidence": evidence,
            }))
            .await?
            .check()?;
        Ok(())
    }

    async fn recent_task_memories(&self, limit: u64) -> Result<Vec<TaskMemory>, VaultError> {
        let mut resp = self
            .db
            .query(
                "SELECT meta::id(id) AS id, request, actions, outcome, \
                        code_refs, session_id, input_tokens, output_tokens, created \
                 FROM task_memory \
                 ORDER BY created DESC \
                 LIMIT $k",
            )
            .bind(json!({ "k": limit as i64 }))
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_task_memory).collect())
    }

    async fn store_test_run(&self, run: TestRun) -> Result<(), VaultError> {
        let code_refs = serde_json::to_value(&run.code_refs)?;
        let outcome = serde_json::to_value(&run.outcome)?;
        self.db
            .query(
                "UPSERT type::record('test_run', $id) CONTENT { \
                   id: $id, task_id: $task_id, code_refs: $code_refs, \
                   outcome: $outcome, created_at: $created_at \
                 }",
            )
            .bind(json!({
                "id":         run.id,
                "task_id":    run.task_id,
                "code_refs":  code_refs,
                "outcome":    outcome,
                "created_at": run.created_at,
            }))
            .await?
            .check()?;
        Ok(())
    }

    async fn test_runs_for_task(&self, task_id: &str) -> Result<Vec<TestRun>, VaultError> {
        let mut resp = self
            .db
            .query(
                "SELECT meta::id(id) AS id, task_id, code_refs, outcome, created_at \
                 FROM test_run \
                 WHERE task_id = $tid \
                 ORDER BY created_at DESC",
            )
            .bind(json!({ "tid": task_id }))
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_test_run).collect())
    }

    // ── R-22 standards + violations ────────────────────────────────────────

    async fn upsert_standard(&self, std: StandardNode) -> Result<(), VaultError> {
        let kind = serde_json::to_value(&std.kind)?;
        let sev  = serde_json::to_value(&std.severity)?;
        self.db
            .query(
                "UPSERT type::record('standard_node', $id) CONTENT { \
                   id: $id, name: $name, kind: $kind, severity: $sev, \
                   rule_text: $rule \
                 }",
            )
            .bind(json!({
                "id":   std.id,
                "name": std.name,
                "kind": kind,
                "sev":  sev,
                "rule": std.rule_text,
            }))
            .await?
            .check()?;
        Ok(())
    }

    async fn list_standards(&self) -> Result<Vec<StandardNode>, VaultError> {
        let mut resp = self.db
            .query(
                "SELECT meta::id(id) AS id, name, kind, severity, rule_text \
                 FROM standard_node ORDER BY id",
            )
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_standard).collect())
    }

    async fn record_violation(&self, v: Violation) -> Result<(), VaultError> {
        let sev = serde_json::to_value(&v.severity)?;
        // Persist the violation row.
        self.db
            .query(
                "UPSERT type::record('violation', $id) CONTENT { \
                   id: $id, code_node_id: $cn, standard_id: $sid, \
                   task_id: $tid, evidence: $ev, severity: $sev, \
                   created_at: $ts \
                 }",
            )
            .bind(json!({
                "id":  v.id,
                "cn":  v.code_node_id,
                "sid": v.standard_id,
                "tid": v.task_id,
                "ev":  v.evidence,
                "sev": sev,
                "ts":  v.created_at,
            }))
            .await?
            .check()?;
        // Best-effort RELATE — skip if endpoint records don't exist (mem-only
        // tests construct violations without seeding the standard/code rows).
        let _ = self.db
            .query(
                "RELATE type::record('code_node', $cn)->violates->\
                 type::record('standard_node', $sid) \
                 SET task_id = $tid, evidence = $ev, severity = $sev"
            )
            .bind(json!({
                "cn": v.code_node_id, "sid": v.standard_id,
                "tid": v.task_id, "ev": v.evidence, "sev": sev,
            }))
            .await;
        Ok(())
    }

    async fn violations_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<Violation>, VaultError> {
        let mut resp = self.db
            .query(
                "SELECT meta::id(id) AS id, code_node_id, standard_id, \
                        task_id, evidence, severity, created_at \
                 FROM violation \
                 WHERE task_id = $tid \
                 ORDER BY created_at DESC",
            )
            .bind(json!({ "tid": task_id }))
            .await?
            .check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(row_to_violation).collect())
    }

    async fn clear_insights(&self) -> Result<u64, VaultError> {
        // Count first so we can report how many were wiped — DELETE in
        // SurrealDB returns nothing meaningful in 3.x.
        let mut count_resp = self.db.query("SELECT count() AS n FROM insight GROUP ALL").await?.check()?;
        let row: Option<Value> = count_resp.take(0)?;
        let before = row.as_ref()
            .and_then(|v| v.get("n"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        self.db.query("DELETE insight").await?.check()?;
        Ok(before)
    }

    async fn clear_session_memory(&self) -> Result<u64, VaultError> {
        let mut count_resp = self.db.query(
            "SELECT count() AS n FROM session_memory GROUP ALL"
        ).await?.check()?;
        let row: Option<Value> = count_resp.take(0)?;
        let before = row.as_ref()
            .and_then(|v| v.get("n"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        self.db.query("DELETE session_memory").await?.check()?;
        Ok(before)
    }

    // ── KB Graph queries ───────────────────────────────────────────────────

    async fn kb_stats(&self) -> Result<crate::store::KbStats, VaultError> {
        let mut resp = self.db.query(
            "LET $files = (SELECT count() AS c FROM file GROUP ALL); \
             LET $nodes = (SELECT count() AS c FROM code_node GROUP ALL); \
             LET $edges = (SELECT count() AS c FROM defines, contains, calls, \
                           imports, references, implements, overrides GROUP ALL); \
             LET $entities = (SELECT count() AS c FROM entity GROUP ALL); \
             LET $insights = (SELECT count() AS c FROM insight GROUP ALL); \
             RETURN { \
               files: $files[0].c OR 0, \
               nodes: $nodes[0].c OR 0, \
               edges: $edges[0].c OR 0, \
               entities: $entities[0].c OR 0, \
               insights: $insights[0].c OR 0 \
             };"
        ).await?.check()?;

        let row: Option<Value> = resp.take(5)?;
        let obj = row.as_ref().and_then(Value::as_object);
        Ok(crate::store::KbStats {
            file_count:    obj.and_then(|o| o.get("files")).and_then(Value::as_u64).unwrap_or(0),
            node_count:    obj.and_then(|o| o.get("nodes")).and_then(Value::as_u64).unwrap_or(0),
            edge_count:    obj.and_then(|o| o.get("edges")).and_then(Value::as_u64).unwrap_or(0),
            entity_count:  obj.and_then(|o| o.get("entities")).and_then(Value::as_u64).unwrap_or(0),
            insight_count: obj.and_then(|o| o.get("insights")).and_then(Value::as_u64).unwrap_or(0),
        })
    }

    async fn kb_files(&self) -> Result<Vec<crate::store::FileWithStats>, VaultError> {
        let mut resp = self.db.query(
            "SELECT path, lang, \
                    (SELECT count() FROM code_node WHERE file_path = $parent.path GROUP ALL)[0].count OR 0 AS node_count \
             FROM file ORDER BY path"
        ).await?.check()?;
        let rows: Vec<Value> = resp.take(0)?;
        Ok(rows.into_iter().filter_map(|v| {
            let obj = v.as_object()?;
            Some(crate::store::FileWithStats {
                path: obj.get("path").and_then(Value::as_str)?.to_string(),
                lang: obj.get("lang").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                node_count: obj.get("node_count").and_then(Value::as_u64).unwrap_or(0),
            })
        }).collect())
    }

    async fn kb_file_graph(&self, file_path: &str) -> Result<crate::store::GraphData, VaultError> {
        // Get all nodes for this file.
        let nodes = self.code_nodes_for_file(file_path).await?;
        let node_ids: std::collections::HashSet<String> =
            nodes.iter().map(|n| n.id.clone()).collect();

        // Get all edges where both endpoints are in this file's node set.
        let mut edges = Vec::new();
        for edge_kind in &[
            CodeEdgeKind::Defines, CodeEdgeKind::Contains, CodeEdgeKind::Calls,
            CodeEdgeKind::Imports, CodeEdgeKind::References, CodeEdgeKind::Implements,
        ] {
            let table = edge_table(edge_kind);
            let (src_tbl, tgt_tbl) = edge_endpoints(edge_kind);
            let sql = format!(
                "SELECT meta::id(in) AS from_id, meta::id(out) AS to_id \
                 FROM {table}"
            );
            let mut resp = self.db.query(&sql).await?.check()?;
            let rows: Vec<Value> = resp.take(0)?;
            for row in rows {
                let obj = match row.as_object() { Some(o) => o, None => continue };
                let from = obj.get("from_id").and_then(Value::as_str).unwrap_or_default();
                let to = obj.get("to_id").and_then(Value::as_str).unwrap_or_default();
                // For DEFINES edges, `from` is a file id — include if file matches.
                let include = if *edge_kind == CodeEdgeKind::Defines {
                    let file_id = sanitize_id(file_path);
                    from == file_id && node_ids.contains(to)
                } else {
                    node_ids.contains(from) || node_ids.contains(to)
                };
                if include {
                    edges.push(CodeEdge {
                        from: from.to_string(),
                        to: to.to_string(),
                        kind: edge_kind.clone(),
                    });
                }
            }
        }

        Ok(crate::store::GraphData { nodes, edges })
    }

    async fn kb_entity_graph(&self) -> Result<crate::store::EntityGraphData, VaultError> {
        let mut resp = self.db.query(
            "SELECT meta::id(id) AS id, name, kind, description FROM entity"
        ).await?.check()?;
        let entity_rows: Vec<Value> = resp.take(0)?;
        let entities: Vec<Entity> = entity_rows.into_iter().filter_map(row_to_entity).collect();

        let mut resp = self.db.query(
            "SELECT meta::id(in) AS source, meta::id(out) AS target, label, weight FROM rel"
        ).await?.check()?;
        let rel_rows: Vec<Value> = resp.take(0)?;
        let relations: Vec<Relation> = rel_rows.into_iter().filter_map(|v| {
            let obj = v.as_object()?;
            Some(Relation {
                source: obj.get("source").and_then(Value::as_str)?.to_string(),
                target: obj.get("target").and_then(Value::as_str)?.to_string(),
                label: obj.get("label").and_then(Value::as_str).unwrap_or_default().to_string(),
                weight: obj.get("weight").and_then(Value::as_f64).unwrap_or(1.0) as f32,
            })
        }).collect();

        Ok(crate::store::EntityGraphData { entities, relations })
    }
}

fn row_to_entity(v: Value) -> Option<Entity> {
    let obj = v.as_object()?;
    Some(Entity {
        id: obj.get("id")?.as_str()?.to_string(),
        name: obj.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
        kind: obj.get("kind").and_then(Value::as_str).unwrap_or_default().to_string(),
        description: obj
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn row_to_vector_match(v: Value) -> Option<VectorMatch> {
    let obj = v.as_object()?;
    let id = obj.get("id")?.as_str()?.to_string();
    let content = obj
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let score = obj.get("score").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let metadata: HashMap<String, Value> = obj
        .get("metadata")
        .and_then(Value::as_object)
        .map(|m| m.clone().into_iter().collect())
        .unwrap_or_default();
    Some(VectorMatch {
        document: VectorDocument {
            id,
            content,
            embedding: None,
            metadata,
        },
        score,
    })
}

fn row_to_code_node(v: Value) -> Option<CodeNode> {
    let obj = v.as_object()?;
    let kind: CodeNodeKind =
        serde_json::from_value(obj.get("kind")?.clone()).ok()?;
    Some(CodeNode {
        id: obj.get("id")?.as_str()?.to_string(),
        file_path: obj.get("file_path").and_then(Value::as_str).unwrap_or_default().to_string(),
        name: obj.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
        kind,
        start_line:  obj.get("start_line").and_then(Value::as_u64).unwrap_or(0) as u32,
        end_line:    obj.get("end_line").and_then(Value::as_u64).unwrap_or(0) as u32,
        preview:     obj.get("preview").and_then(Value::as_str).unwrap_or_default().to_string(),
        visibility:  obj.get("visibility").and_then(Value::as_str).unwrap_or_default().to_string(),
        qualifiers:  obj.get("qualifiers").and_then(Value::as_str).unwrap_or_default().to_string(),
        description: obj.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
    })
}

/// Map `CodeEdgeKind` to the SurrealDB relation table name.
fn edge_table(kind: &CodeEdgeKind) -> &'static str {
    match kind {
        CodeEdgeKind::Defines    => "defines",
        CodeEdgeKind::Contains   => "contains",
        CodeEdgeKind::Calls      => "calls",
        CodeEdgeKind::Imports    => "imports",
        CodeEdgeKind::References => "references",
        CodeEdgeKind::Implements => "implements",
        CodeEdgeKind::Overrides  => "overrides",
        CodeEdgeKind::Violates   => "violates",
    }
}

/// Map `CodeEdgeKind` to (source_table, target_table) for RELATE.
fn edge_endpoints(kind: &CodeEdgeKind) -> (&'static str, &'static str) {
    match kind {
        CodeEdgeKind::Defines  => ("file", "code_node"),
        CodeEdgeKind::Violates => ("code_node", "standard_node"),
        _                      => ("code_node", "code_node"),
    }
}

fn row_to_commit(v: Value) -> Option<GitCommit> {
    let obj = v.as_object()?;
    let files = obj.get("files_changed")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    Some(GitCommit {
        hash:          obj.get("hash")?.as_str()?.to_string(),
        short_hash:    obj.get("short_hash")?.as_str()?.to_string(),
        message:       obj.get("message")?.as_str()?.to_string(),
        author_name:   obj.get("author_name")?.as_str()?.to_string(),
        author_email:  obj.get("author_email")?.as_str()?.to_string(),
        timestamp:     obj.get("timestamp")?.as_i64()?,
        files_changed: files,
        insertions:    obj.get("insertions").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        deletions:     obj.get("deletions").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
    })
}

/// Turn an arbitrary file path into a SurrealDB-safe record id segment.
pub fn sanitize_id(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect()
}

fn row_to_task_memory(v: Value) -> Option<TaskMemory> {
    let obj = v.as_object()?;
    let outcome: TaskOutcome =
        serde_json::from_value(obj.get("outcome")?.clone()).ok()?;
    let actions: Vec<String> =
        serde_json::from_value(obj.get("actions")?.clone()).unwrap_or_default();
    let code_refs: Vec<String> =
        serde_json::from_value(obj.get("code_refs")?.clone()).unwrap_or_default();
    Some(TaskMemory {
        id:            obj.get("id")?.as_str()?.to_string(),
        request:       obj.get("request").and_then(Value::as_str).unwrap_or_default().to_string(),
        actions,
        outcome,
        code_refs,
        session_id:    obj.get("session_id").and_then(Value::as_str).unwrap_or_default().to_string(),
        input_tokens:  obj.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
        output_tokens: obj.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
    })
}

fn row_to_standard(v: Value) -> Option<StandardNode> {
    let obj  = v.as_object()?;
    let kind: StandardKind = serde_json::from_value(obj.get("kind")?.clone()).ok()?;
    let sev:  Severity     = serde_json::from_value(obj.get("severity")?.clone()).ok()?;
    Some(StandardNode {
        id:        obj.get("id")?.as_str()?.to_string(),
        name:      obj.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
        kind,
        severity:  sev,
        rule_text: obj.get("rule_text").and_then(Value::as_str).unwrap_or_default().to_string(),
    })
}

fn row_to_violation(v: Value) -> Option<Violation> {
    let obj = v.as_object()?;
    let sev: Severity = serde_json::from_value(obj.get("severity")?.clone()).ok()?;
    Some(Violation {
        id:           obj.get("id")?.as_str()?.to_string(),
        code_node_id: obj.get("code_node_id").and_then(Value::as_str).unwrap_or_default().to_string(),
        standard_id:  obj.get("standard_id").and_then(Value::as_str).unwrap_or_default().to_string(),
        task_id:      obj.get("task_id").and_then(Value::as_str).unwrap_or_default().to_string(),
        evidence:     obj.get("evidence").and_then(Value::as_str).unwrap_or_default().to_string(),
        severity:     sev,
        created_at:   obj.get("created_at").and_then(Value::as_i64).unwrap_or(0),
    })
}

fn row_to_test_run(v: Value) -> Option<TestRun> {
    let obj = v.as_object()?;
    let outcome: ExecutionOutcome =
        serde_json::from_value(obj.get("outcome")?.clone()).ok()?;
    let code_refs: Vec<String> =
        serde_json::from_value(obj.get("code_refs")?.clone()).unwrap_or_default();
    Some(TestRun {
        id:         obj.get("id")?.as_str()?.to_string(),
        task_id:    obj.get("task_id").and_then(Value::as_str).unwrap_or_default().to_string(),
        code_refs,
        outcome,
        created_at: obj.get("created_at").and_then(Value::as_i64).unwrap_or(0),
    })
}

fn row_to_insight(v: Value) -> Option<Insight> {
    let obj = v.as_object()?;
    let kind: InsightKind =
        serde_json::from_value(obj.get("kind")?.clone()).ok()?;
    let evidence: Vec<String> =
        serde_json::from_value(obj.get("evidence")?.clone()).unwrap_or_default();
    Some(Insight {
        id:       obj.get("id")?.as_str()?.to_string(),
        kind,
        scope:    obj.get("scope").and_then(Value::as_str).unwrap_or_default().to_string(),
        summary:  obj.get("summary").and_then(Value::as_str).unwrap_or_default().to_string(),
        evidence,
    })
}
