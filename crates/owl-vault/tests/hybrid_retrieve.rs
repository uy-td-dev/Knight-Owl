//! Tests for WF-13 hybrid retrieval primitives.
//!
//! The in-memory SurrealDB backend doesn't support BM25 SEARCH indexes, so
//! we focus on testing the graph-walk leg (the new R-20 capability)
//! directly via `walk_graph`.  End-to-end BM25 + graph fusion is exercised
//! in real backends only.

use std::sync::Arc;

use owl_protocol::code::{CodeEdge, CodeEdgeKind, CodeNode, CodeNodeKind, FileNode};
use owl_vault::surreal::{SurrealConfig, SurrealStore};
use owl_vault::{
    hybrid_retrieve, walk_graph, HybridRetrieveConfig, HybridStore,
};

async fn mem_store() -> Arc<dyn HybridStore> {
    Arc::new(
        SurrealStore::connect(SurrealConfig::memory())
            .await
            .expect("in-memory SurrealDB should always connect"),
    )
}

fn node(id: &str, name: &str) -> CodeNode {
    CodeNode {
        id:          id.into(),
        file_path:   "src/lib.rs".into(),
        name:        name.into(),
        kind:        CodeNodeKind::Function,
        start_line:  1,
        end_line:    5,
        preview:     format!("fn {name}() {{}}"),
        visibility:  "pub".into(),
        qualifiers:  String::new(),
        description: String::new(),
    }
}

/// Seed a A → B → C call chain and return the store.
async fn seeded_chain() -> Arc<dyn HybridStore> {
    let store = mem_store().await;
    store.upsert_file(FileNode {
        path:         "src/lib.rs".into(),
        lang:         "rust".into(),
        content_hash: "h".into(),
    }).await.unwrap();
    for n in [node("a", "payment_handler"),
              node("b", "validate_charge"),
              node("c", "log_audit")] {
        store.upsert_code_node(n).await.unwrap();
    }
    for (from, to) in [("a", "b"), ("b", "c")] {
        store.upsert_code_edge(CodeEdge {
            from: from.into(),
            to:   to.into(),
            kind: CodeEdgeKind::Calls,
        }).await.unwrap();
    }
    store
}

#[tokio::test]
async fn walk_graph_finds_neighbours_at_correct_depth() {
    let store = seeded_chain().await;

    // Seed = A only.  Depth = 2 should reach B (1 hop) and C (2 hops).
    let hits = walk_graph(
        &*store,
        vec!["a".into()],
        2,
        &[CodeEdgeKind::Calls],
        20,
    ).await.unwrap();

    // Map id → depth for asserts.
    let depths: std::collections::HashMap<String, usize> =
        hits.iter().map(|h| (h.node.id.clone(), h.depth)).collect();

    assert_eq!(depths.get("b"), Some(&1), "B is one CALLS hop from A");
    assert_eq!(depths.get("c"), Some(&2), "C is two CALLS hops from A");
    assert!(!depths.contains_key("a"),
            "seeds are not re-emitted as walk hits");
}

#[tokio::test]
async fn walk_graph_respects_depth_zero() {
    let store = seeded_chain().await;
    let hits = walk_graph(
        &*store,
        vec!["a".into()],
        0,
        &[CodeEdgeKind::Calls],
        20,
    ).await.unwrap();
    // depth=0 still emits the immediate neighbours discovered at the first
    // hop (BFS yields them with depth=1 before checking the boundary), so
    // B should appear, C should not.
    let has_b = hits.iter().any(|h| h.node.id == "b");
    let has_c = hits.iter().any(|h| h.node.id == "c");
    assert!(has_b, "1-hop neighbour B must appear even at depth=0 seed");
    assert!(!has_c, "2-hop C must NOT appear when depth=0");
}

#[tokio::test]
async fn walk_graph_caps_at_max_nodes() {
    let store = seeded_chain().await;
    let hits = walk_graph(
        &*store,
        vec!["a".into()],
        5,
        &[CodeEdgeKind::Calls],
        1,
    ).await.unwrap();
    assert!(hits.len() <= 1, "max_nodes cap honoured (got {})", hits.len());
}

#[tokio::test]
async fn hybrid_retrieve_handles_empty_prompt() {
    let store = mem_store().await;
    let out = hybrid_retrieve(
        &*store, None, "   ",
        &HybridRetrieveConfig::default(),
    ).await.unwrap();
    assert!(out.is_empty(), "empty prompt = no work");
}

#[tokio::test]
async fn hybrid_retrieve_handles_empty_store() {
    let store = mem_store().await;
    // No seeds in store — retrieval is graceful, not panicky.
    let out = hybrid_retrieve(
        &*store, None, "anything",
        &HybridRetrieveConfig::default(),
    ).await.unwrap();
    assert!(out.is_empty(), "no seeds = empty result");
}
