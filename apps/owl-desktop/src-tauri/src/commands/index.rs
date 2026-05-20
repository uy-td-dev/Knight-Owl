//! Workspace indexing commands.
//!
//! Walks the active workspace, parses every file owl-cortex can handle
//! (`.rs`, `.ts`/`.tsx`, `.py` per cargo features), and persists the AST
//! into SurrealDB as L1 nodes + edges (`file`, `code_node`, `defines`,
//! `contains`, `calls`, …).
//!
//! Progress is streamed to the frontend as `index_progress` events so the
//! UI can render a progress bar.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use owl_cortex::ingest::Ingestor;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use walkdir::{DirEntry, WalkDir};

use crate::state::AppState;

const IGNORE_DIRS: &[&str] = &[
    ".git", "target", "node_modules", "dist", "build",
    ".next", ".venv", ".turbo", ".cache", "vendor",
];

const SUPPORTED_EXT: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "mjs", "cjs",
];

#[derive(Clone, Serialize)]
pub struct IndexProgress {
    pub total:   usize,
    pub indexed: usize,
    pub current: String,
    pub nodes:   usize,
    pub done:    bool,
    pub error:   Option<String>,
}

/// Tracks whether an index run is currently active so we can refuse
/// concurrent invocations without holding a mutex across awaits.
static INDEXING: AtomicBool = AtomicBool::new(false);

/// Walk the active workspace and ingest every supported source file.
///
/// Returns immediately; results stream over the `index_progress` event.
#[tauri::command]
pub async fn index_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let store = state.store.clone()
        .ok_or_else(|| "SurrealDB unreachable — start the database first".to_string())?;
    let workspace = state.workspace.clone();
    let embedder = Arc::clone(&state.embedder);
    let cartographer = state.cartographer.clone();

    if INDEXING.swap(true, Ordering::SeqCst) {
        return Err("Indexing already in progress.".into());
    }

    tauri::async_runtime::spawn(async move {
        let _guard = IndexGuard;
        let result = run_index(app.clone(), workspace, store, embedder, cartographer).await;
        if let Err(e) = result {
            let _ = app.emit("index_progress", IndexProgress {
                total: 0, indexed: 0, current: String::new(),
                nodes: 0, done: true, error: Some(e),
            });
        }
    });

    Ok(())
}

struct IndexGuard;
impl Drop for IndexGuard {
    fn drop(&mut self) { INDEXING.store(false, Ordering::SeqCst); }
}

async fn run_index(
    app: AppHandle,
    workspace: PathBuf,
    store: Arc<dyn owl_vault::HybridStore>,
    embedder: Arc<dyn owl_vault::Embedder>,
    cartographer: Option<Arc<dyn owl_cartographer::GraphIngester>>,
) -> Result<(), String> {
    // Phase 1: enumerate candidate files.
    let candidates = collect_files(&workspace);
    let total = candidates.len();
    tracing::info!(workspace = %workspace.display(), total, "starting workspace index");

    let _ = app.emit("index_progress", IndexProgress {
        total, indexed: 0, current: "scanning…".into(),
        nodes: 0, done: false, error: None,
    });

    let cortex_cfg = owl_cortex::Config::load().unwrap_or_default();
    let ingestor = Ingestor::new(Arc::clone(&store), cortex_cfg);
    let mut indexed_count = 0usize;
    let mut node_count    = 0usize;

    for path in &candidates {
        let rel = path.strip_prefix(&workspace).unwrap_or(path).display().to_string();
        match ingestor.on_file_changed(path, &workspace).await {
            Ok(n)  => {
                node_count    += n;
                indexed_count += 1;
            }
            Err(e) => {
                tracing::warn!(file = %rel, err = %e, "ingest failed");
            }
        }

        if indexed_count % 5 == 0 || indexed_count == total {
            let _ = app.emit("index_progress", IndexProgress {
                total, indexed: indexed_count,
                current: rel, nodes: node_count,
                done: false, error: None,
            });
        }
    }

    // Phase 2: embed code_nodes that don't yet have embeddings.
    let _ = app.emit("index_progress", IndexProgress {
        total, indexed: indexed_count,
        current: "Embedding code nodes…".into(),
        nodes: node_count, done: false, error: None,
    });

    let mut embedded = 0usize;
    for path in &candidates {
        let rel = path.strip_prefix(&workspace).unwrap_or(path).display().to_string();
        let nodes = match store.code_nodes_for_file(&rel).await {
            Ok(n) => n,
            Err(_) => continue,
        };
        for node in nodes {
            let text = format!("{} {} {}", node.name, node.description, node.preview);
            match embedder.embed(&text).await {
                Ok(emb) => {
                    if let Err(e) = store.update_code_node_embedding(&node.id, emb.values).await {
                        tracing::warn!(id = %node.id, err = %e, "embed update failed");
                    } else {
                        embedded += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!(id = %node.id, err = %e, "embedding failed");
                }
            }
        }
    }
    tracing::info!(embedded, "code node embeddings complete");

    // Phase 3: GraphRAG entity extraction from module-level doc comments.
    let mut entities_extracted = 0usize;
    if let Some(carto) = cartographer {
        let _ = app.emit("index_progress", IndexProgress {
            total, indexed: indexed_count,
            current: "Extracting L3 entities from doc comments…".into(),
            nodes: node_count, done: false, error: None,
        });

        for path in &candidates {
            let rel = path.strip_prefix(&workspace).unwrap_or(path).display().to_string();
            let nodes = match store.code_nodes_for_file(&rel).await {
                Ok(n) => n,
                Err(_) => continue,
            };
            let doc_chunks: Vec<String> = nodes
                .iter()
                .filter(|n| !n.description.is_empty())
                .map(|n| format!("[{}:{}] {} — {}", rel, n.start_line, n.name, n.description))
                .collect();

            if doc_chunks.is_empty() { continue; }

            let chunk = doc_chunks.join("\n");
            match carto.ingest(&chunk).await {
                Ok(count) => entities_extracted += count,
                Err(e) => tracing::warn!(file = %rel, err = %e, "cartographer ingest failed"),
            }
        }
        tracing::info!(entities_extracted, "L3 entity extraction complete");
    }

    let _ = app.emit("index_progress", IndexProgress {
        total, indexed: indexed_count,
        current: format!("Indexed {indexed_count}/{total} files, {node_count} nodes, {embedded} embedded, {entities_extracted} entities"),
        nodes: node_count, done: true, error: None,
    });

    tracing::info!(indexed = indexed_count, nodes = node_count, "index complete");
    Ok(())
}

fn collect_files(root: &PathBuf) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_ignored(e))
    {
        let entry = match entry { Ok(e) => e, Err(_) => continue };
        if !entry.file_type().is_file() { continue; }
        let ext = entry.path().extension().and_then(|s| s.to_str()).unwrap_or("");
        if SUPPORTED_EXT.contains(&ext) {
            out.push(entry.into_path());
        }
    }
    out
}

fn is_ignored(entry: &DirEntry) -> bool {
    if entry.file_type().is_dir() {
        if let Some(name) = entry.path().file_name().and_then(|s| s.to_str()) {
            if IGNORE_DIRS.contains(&name) { return true; }
            // hidden dotfiles, but keep the workspace root itself
            if name.starts_with('.') && entry.depth() > 0 { return true; }
        }
    }
    false
}
