//! Knowledge Base query commands — expose the graph to the frontend.

use std::sync::Arc;

use tauri::State;

use owl_protocol::code::CodeNode;
use owl_protocol::experience::Insight;
use owl_vault::store::{EntityGraphData, FileWithStats, GraphData, KbStats};

use crate::state::AppState;

fn store(state: &AppState) -> Result<Arc<dyn owl_vault::HybridStore>, String> {
    state.store.clone().ok_or_else(|| "SurrealDB not connected".into())
}

#[tauri::command]
pub async fn kb_stats(state: State<'_, AppState>) -> Result<KbStats, String> {
    store(&state)?.kb_stats().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kb_files(state: State<'_, AppState>) -> Result<Vec<FileWithStats>, String> {
    store(&state)?.kb_files().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kb_file_graph(
    path: String,
    state: State<'_, AppState>,
) -> Result<GraphData, String> {
    store(&state)?.kb_file_graph(&path).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kb_node_neighbors(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<CodeNode>, String> {
    let s = store(&state)?;
    let mut results = Vec::new();
    for kind in &[
        owl_protocol::code::CodeEdgeKind::Calls,
        owl_protocol::code::CodeEdgeKind::Contains,
        owl_protocol::code::CodeEdgeKind::References,
        owl_protocol::code::CodeEdgeKind::Implements,
    ] {
        match s.code_edges_from(&id, kind.clone()).await {
            Ok(nodes) => results.extend(nodes),
            Err(_) => {}
        }
    }
    Ok(results)
}

#[tauri::command]
pub async fn kb_search(
    query: String,
    mode: Option<String>,
    limit: Option<u64>,
    state: State<'_, AppState>,
) -> Result<Vec<CodeNode>, String> {
    let s = store(&state)?;
    let k = limit.unwrap_or(20);

    match mode.as_deref().unwrap_or("hybrid") {
        "keyword" => s.keyword_search(&query, k).await.map_err(|e| e.to_string()),
        "semantic" => {
            let emb = state.embedder.embed(&query).await.map_err(|e| e.to_string())?;
            s.vector_search_code_nodes(emb.values, k).await.map_err(|e| e.to_string())
        }
        _ => {
            // Hybrid: BM25 + vector, deduplicated by id.
            let kw = s.keyword_search(&query, k).await.unwrap_or_default();
            let sem = match state.embedder.embed(&query).await {
                Ok(emb) => s.vector_search_code_nodes(emb.values, k).await.unwrap_or_default(),
                Err(_) => vec![],
            };

            let mut seen = std::collections::HashSet::new();
            let mut merged = Vec::new();
            for node in kw.into_iter().chain(sem.into_iter()) {
                if seen.insert(node.id.clone()) {
                    merged.push(node);
                }
            }
            merged.truncate(k as usize);
            Ok(merged)
        }
    }
}

#[tauri::command]
pub async fn kb_insights(
    scope: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Insight>, String> {
    let s = store(&state)?;
    let prefix = scope.as_deref().unwrap_or("global");
    s.recall_insights(prefix).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kb_entities(
    state: State<'_, AppState>,
) -> Result<EntityGraphData, String> {
    store(&state)?.kb_entity_graph().await.map_err(|e| e.to_string())
}

#[derive(Clone, serde::Serialize)]
pub struct QaContext {
    pub citations: Vec<Citation>,
    pub insights: Vec<Insight>,
}

#[derive(Clone, serde::Serialize)]
pub struct Citation {
    pub file_path: String,
    pub name: String,
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub preview: String,
    pub description: String,
}

/// Retrieve grounded context from the KB for a question.
///
/// Returns code citations + relevant insights that can be used to build
/// an LLM prompt or displayed directly in the UI.
#[tauri::command]
pub async fn kb_ask(
    question: String,
    state: State<'_, AppState>,
) -> Result<QaContext, String> {
    let s = store(&state)?;
    let k: u64 = 10;

    // Hybrid retrieval: BM25 + vector
    let kw = s.keyword_search(&question, k).await.unwrap_or_default();
    let sem = match state.embedder.embed(&question).await {
        Ok(emb) => s.vector_search_code_nodes(emb.values, k).await.unwrap_or_default(),
        Err(_) => vec![],
    };

    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::new();
    for node in kw.into_iter().chain(sem.into_iter()) {
        if seen.insert(node.id.clone()) {
            merged.push(node);
        }
    }
    merged.truncate(k as usize);

    // Graph expansion: for top-5 results, fetch 1-hop neighbors.
    // Snapshot the seed ids first so we can mutate `merged` in the loop body
    // without holding an immutable borrow across the await/push.
    let mut expanded_ids = std::collections::HashSet::new();
    let seed_ids: Vec<String> = merged.iter().take(5).map(|n| n.id.clone()).collect();
    for seed_id in &seed_ids {
        expanded_ids.insert(seed_id.clone());
        for kind in &[
            owl_protocol::code::CodeEdgeKind::Calls,
            owl_protocol::code::CodeEdgeKind::References,
        ] {
            if let Ok(neighbors) = s.code_edges_from(seed_id, kind.clone()).await {
                for n in neighbors.into_iter().take(3) {
                    if expanded_ids.insert(n.id.clone()) {
                        merged.push(n);
                    }
                }
            }
        }
    }
    merged.truncate(20);

    let citations: Vec<Citation> = merged
        .into_iter()
        .map(|n| Citation {
            file_path: n.file_path,
            name: n.name,
            kind: format!("{:?}", n.kind),
            start_line: n.start_line,
            end_line: n.end_line,
            preview: n.preview,
            description: n.description,
        })
        .collect();

    let insights = s.recall_insights("global").await.unwrap_or_default();

    Ok(QaContext { citations, insights })
}

#[tauri::command]
pub async fn kb_distill(
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let s = store(&state)?;
    let exp = owl_vault::SurrealExperienceStore::new(s);
    let distiller = owl_vault::Distiller::new(std::sync::Arc::new(exp));
    distiller.run().await.map_err(|e| e.to_string())
}
