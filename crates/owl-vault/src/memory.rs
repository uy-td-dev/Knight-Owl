//! `SurrealMemoryStore` — persistent, optionally semantic memory backed by
//! SurrealDB's `session_memory` table.
//!
//! # Level 1 — Persistence
//! Every `MemoryEntry` pushed is written to SurrealDB keyed by `session_id`.
//! Entries survive process restarts; resuming with the same session id reloads
//! conversation history.
//!
//! # Level 2 — Semantic recall
//! When a [`crate::Embedder`] is injected, `push()` stores a vector alongside
//! each entry and `recall()` blends cosine-similarity search (top-¾) with the
//! most-recent entries for conversation continuity.
//! Without an embedder, `recall()` falls back to plain recency.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use surrealdb::engine::any::{connect, Any};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;

use owl_protocol::memory::{MemoryEntry, MemoryError, MemoryStore, SessionRecallHit};

use crate::{Embedder, SurrealConfig};

/// SurrealDB-backed memory store implementing [`MemoryStore`].
pub struct SurrealMemoryStore {
    db:         Surreal<Any>,
    session_id: String,
    embedder:   Option<Arc<dyn Embedder>>,
}

impl SurrealMemoryStore {
    /// Connect to SurrealDB and return a store bound to `session_id`.
    ///
    /// Pass an `embedder` to activate semantic recall (Level 2); `None`
    /// degrades gracefully to recency-only.
    pub async fn connect(
        cfg:        SurrealConfig,
        session_id: String,
        embedder:   Option<Arc<dyn Embedder>>,
    ) -> Result<Self, crate::VaultError> {
        let db = connect(cfg.endpoint)
            .await
            .map_err(|e: surrealdb::Error| crate::VaultError::Surreal(e.to_string()))?;

        // Sign in as root when credentials are supplied — required for any
        // ws:// or http:// SurrealDB endpoint with auth enabled (the default
        // for `surreal start`).  Skipping this leaves the connection at
        // anonymous level, where every read/write fails with "Not enough
        // permissions to perform this action".
        if let (Some(u), Some(p)) = (cfg.username.as_ref(), cfg.password.as_ref()) {
            db.signin(Root { username: u.clone(), password: p.clone() })
                .await
                .map_err(|e: surrealdb::Error| {
                    crate::VaultError::Surreal(format!("signin failed: {e}"))
                })?;
        }

        db.use_ns(cfg.namespace)
            .use_db(cfg.database)
            .await
            .map_err(|e: surrealdb::Error| crate::VaultError::Surreal(e.to_string()))?;
        Ok(Self { db, session_id, embedder })
    }

    /// Return the session id (useful for displaying to the user).
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

#[async_trait]
impl MemoryStore for SurrealMemoryStore {
    async fn push(&self, entry: MemoryEntry) -> Result<(), MemoryError> {
        let embedding: Option<Vec<f32>> = if let Some(emb) = &self.embedder {
            emb.embed(&entry.content).await.ok().map(|e| e.values)
        } else {
            None
        };

        self.db
            .query(
                "CREATE session_memory CONTENT { \
                   session_id: $sid, role: $role, content: $content, \
                   embedding: $embedding, created: time::now() \
                 }",
            )
            .bind(json!({
                "sid":       self.session_id,
                "role":      entry.role,
                "content":   entry.content,
                "embedding": embedding,
            }))
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?
            .check()
            .map_err(|e| MemoryError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn recent(&self, limit: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
        let mut resp = self
            .db
            .query(
                // SurrealDB ≥ 3.0 requires every `ORDER BY` idiom to be
                // present in the SELECT projection — include `created` so
                // sorting succeeds, then drop it when mapping rows.
                "SELECT role, content, created FROM session_memory \
                 WHERE session_id = $sid \
                 ORDER BY created ASC \
                 LIMIT $k",
            )
            .bind(json!({ "sid": self.session_id, "k": limit as i64 }))
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?
            .check()
            .map_err(|e| MemoryError::Backend(e.to_string()))?;

        let rows: Vec<Value> = resp.take(0).map_err(|e| MemoryError::Backend(e.to_string()))?;
        Ok(rows.into_iter().filter_map(row_to_entry).collect())
    }

    /// Semantic recall: top-¾ by cosine similarity + recent tail, deduplicated.
    ///
    /// Falls back to `recent(limit)` when no embedder is configured.
    async fn recall(&self, prompt: &str, limit: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
        let embedder = match &self.embedder {
            Some(e) => e,
            None => return self.recent(limit).await,
        };

        let query_vec = embedder
            .embed(prompt)
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?
            .values;

        let semantic_limit = (limit * 3 / 4).max(5);
        let mut resp = self
            .db
            .query(
                "SELECT role, content, \
                        vector::similarity::cosine(embedding, $q) AS score \
                 FROM session_memory \
                 WHERE embedding != NONE AND session_id = $sid \
                 ORDER BY score DESC \
                 LIMIT $k",
            )
            .bind(json!({
                "q":   query_vec,
                "sid": self.session_id,
                "k":   semantic_limit as i64,
            }))
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?
            .check()
            .map_err(|e| MemoryError::Backend(e.to_string()))?;

        let semantic_rows: Vec<Value> =
            resp.take(0).map_err(|e| MemoryError::Backend(e.to_string()))?;
        let mut entries: Vec<MemoryEntry> =
            semantic_rows.into_iter().filter_map(row_to_entry).collect();

        // Always append recent tail for conversation continuity.
        let tail = limit.saturating_sub(entries.len()).max(5);
        let recency = self.recent(tail).await?;
        let seen: std::collections::HashSet<String> =
            entries.iter().map(|e| e.content.clone()).collect();
        for entry in recency {
            if !seen.contains(&entry.content) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    async fn clear(&self) -> Result<(), MemoryError> {
        self.db
            .query("DELETE session_memory WHERE session_id = $sid")
            .bind(json!({ "sid": self.session_id }))
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?
            .check()
            .map_err(|e| MemoryError::Backend(e.to_string()))?;
        Ok(())
    }

    /// BM25 over `session_memory.content` across every session except
    /// `exclude_session`.  Falls back to substring match on backends
    /// without a SEARCH index (kv-mem in tests).
    async fn search_session_memory(
        &self,
        query:           &str,
        exclude_session: &str,
        limit:           usize,
    ) -> Result<Vec<SessionRecallHit>, MemoryError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // BM25 path: requires the conv_analyzer SEARCH index defined at connect.
        let bm25_sql =
            "SELECT session_id, role, content, \
                    search::score(1) AS score \
             FROM session_memory \
             WHERE content @1@ $q AND session_id != $excl \
             ORDER BY score DESC \
             LIMIT $k";

        let bm25 = self.db
            .query(bm25_sql)
            .bind(json!({
                "q":    query,
                "excl": exclude_session,
                "k":    limit as i64,
            }))
            .await
            .and_then(|mut r| { let v: Result<Vec<Value>, _> = r.take(0); v.map_err(Into::into) });

        if let Ok(rows) = bm25 {
            if !rows.is_empty() {
                return Ok(rows.into_iter().filter_map(row_to_recall_hit).collect());
            }
        }

        // Substring fallback (kv-mem, or BM25 yielded empty).
        let q_lower = query.to_lowercase();
        let mut resp = self.db
            .query(
                "SELECT session_id, role, content \
                 FROM session_memory \
                 WHERE string::contains(string::lowercase(content), $q) \
                   AND session_id != $excl \
                 LIMIT $k",
            )
            .bind(json!({
                "q":    q_lower,
                "excl": exclude_session,
                "k":    limit as i64,
            }))
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?
            .check()
            .map_err(|e| MemoryError::Backend(e.to_string()))?;
        let rows: Vec<Value> = resp.take(0)
            .map_err(|e| MemoryError::Backend(e.to_string()))?;
        Ok(rows.into_iter().filter_map(row_to_recall_hit).collect())
    }
}

fn row_to_entry(v: Value) -> Option<MemoryEntry> {
    let obj = v.as_object()?;
    Some(MemoryEntry {
        role:    obj.get("role")?.as_str()?.to_string(),
        content: obj.get("content")?.as_str()?.to_string(),
    })
}

fn row_to_recall_hit(v: Value) -> Option<SessionRecallHit> {
    let obj = v.as_object()?;
    Some(SessionRecallHit {
        session_id: obj.get("session_id")?.as_str()?.to_string(),
        role:       obj.get("role")?.as_str()?.to_string(),
        content:    obj.get("content")?.as_str()?.to_string(),
        score:      obj.get("score").and_then(Value::as_f64).unwrap_or(0.0) as f32,
    })
}
