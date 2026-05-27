//! Phase G — cross-session FTS recall via `SurrealMemoryStore`.
//!
//! Seeds two sessions, queries one, asserts hits come from the OTHER
//! session only.  Uses kv-mem backend → exercises the substring fallback
//! (BM25 SEARCH indexes don't exist on kv-mem); the same code path is
//! used in production by the rocksdb backend.

use std::sync::Arc;

use owl_protocol::memory::{MemoryEntry, MemoryStore};
use owl_vault::surreal::SurrealConfig;
use owl_vault::SurrealMemoryStore;

async fn store_for(session: &str) -> Arc<dyn MemoryStore> {
    let cfg = SurrealConfig::memory();
    Arc::new(
        SurrealMemoryStore::connect(cfg, session.to_string(), None)
            .await
            .expect("in-memory SurrealDB should connect"),
    )
}

#[tokio::test]
#[ignore] // kv-mem doesn't share namespace across SurrealStore::connect; tracked.
async fn cross_session_recall_excludes_current_session() {
    // Seed session "old" with a unique keyword.
    let old = store_for("old").await;
    old.push(MemoryEntry {
        role:    "user".into(),
        content: "we discussed deploying with kubernetes last week".into(),
    }).await.unwrap();

    // Seed session "new" with unrelated content.
    let new = store_for("new").await;
    new.push(MemoryEntry {
        role:    "user".into(),
        content: "rewrite the tokio reactor".into(),
    }).await.unwrap();

    // Query from "new"'s perspective.
    let hits = new
        .search_session_memory("kubernetes", "new", 10)
        .await
        .expect("recall");

    assert!(hits.iter().any(|h| h.content.contains("kubernetes")),
            "old-session hit must surface");
    assert!(!hits.iter().any(|h| h.session_id == "new"),
            "current session excluded");
}

#[tokio::test]
async fn empty_query_returns_empty() {
    let s = store_for("any").await;
    let hits = s.search_session_memory("   ", "any", 10).await.unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn excludes_current_session_id() {
    let s = store_for("current").await;
    s.push(MemoryEntry {
        role:    "user".into(),
        content: "we use postgres for everything".into(),
    }).await.unwrap();
    let hits = s.search_session_memory("postgres", "current", 10).await.unwrap();
    // Same session_id as exclude — must not show up.
    assert!(hits.iter().all(|h| h.session_id != "current"),
            "current session must be filtered out");
}
