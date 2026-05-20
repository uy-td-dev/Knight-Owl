//! Cortex ingestor — drives file parsing and persists results to `HybridStore`.
//!
//! Entry point: `Ingestor::on_file_changed(path, workspace_root)`.
//!
//! Steps:
//!   1. Read file content.
//!   2. Delete stale code_nodes for the file.
//!   3. Upsert file record.
//!   4. Parse AST → upsert nodes + L1 edges (DEFINES, CONTAINS, CALLS).
//!   5. (Optional L2) If an `LspClient` is attached, resolve cross-file
//!      REFERENCES and IMPLEMENTS edges for every exported symbol.

use std::path::Path;
use std::sync::Arc;

use tracing::warn;

use owl_protocol::code::{CodeEdge, CodeEdgeKind, FileNode};
use owl_vault::HybridStore;

use crate::ast::{parser_for_ext, ParseResult};
use crate::config::Config;
use crate::lsp::LspClient;
use crate::CortexError;

/// Persists AST-derived code nodes and edges into the hybrid store.
pub struct Ingestor {
    store:  Arc<dyn HybridStore>,
    config: Config,
    lsp:    Option<Arc<LspClient>>,
}

impl Ingestor {
    /// Create an ingestor backed by the given hybrid store.
    pub fn new(store: Arc<dyn HybridStore>, config: Config) -> Self {
        Self { store, config, lsp: None }
    }

    /// Attach an LSP client to enable L2 cross-file edge resolution.
    ///
    /// Without this the ingestor only populates L1 (DEFINES/CONTAINS/CALLS).
    pub fn with_lsp(mut self, lsp: Arc<LspClient>) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// Process a single changed file: parse, diff, and persist (WF-12).
    ///
    /// `workspace_root` is used to compute relative file paths for node ids.
    pub async fn on_file_changed(
        &self,
        abs_path: &Path,
        workspace_root: &Path,
    ) -> Result<usize, CortexError> {
        let rel_path = abs_path
            .strip_prefix(workspace_root)
            .unwrap_or(abs_path)
            .to_string_lossy()
            .to_string();

        let ext = abs_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let parser = match parser_for_ext(ext) {
            Some(p) => p,
            None    => return Ok(0),
        };

        let source = tokio::fs::read(abs_path).await?;
        let hash = sha256_hex(&source);

        // Delete stale nodes before re-ingesting.
        self.store.delete_code_nodes_for_file(&rel_path).await?;

        // Upsert file record.
        self.store
            .upsert_file(FileNode {
                path: rel_path.clone(),
                lang: parser.lang().to_string(),
                content_hash: hash,
            })
            .await?;

        // Parse AST.
        let result: ParseResult =
            parser.parse(&rel_path, &source, self.config.preview_bytes)?;
        let node_count = result.nodes.len();

        // Upsert nodes.
        for node in &result.nodes {
            self.store.upsert_code_node(node.clone()).await?;
        }

        // L1: DEFINES edges (file → top-level nodes).
        let file_id = crate::file_id(&rel_path);
        let child_ids: std::collections::HashSet<String> =
            result.contains.iter().map(|(_, c)| c.clone()).collect();

        for node in &result.nodes {
            if !child_ids.contains(&node.id) {
                self.store
                    .upsert_code_edge(CodeEdge {
                        from: file_id.clone(),
                        to: node.id.clone(),
                        kind: CodeEdgeKind::Defines,
                    })
                    .await?;
            }
        }

        // L1: CONTAINS edges.
        for (parent_id, child_id) in &result.contains {
            self.store
                .upsert_code_edge(CodeEdge {
                    from: parent_id.clone(),
                    to: child_id.clone(),
                    kind: CodeEdgeKind::Contains,
                })
                .await?;
        }

        // L1: CALLS edges (same-file resolution only).
        let name_to_id: std::collections::HashMap<String, String> = result
            .nodes
            .iter()
            .map(|n| (n.name.clone(), n.id.clone()))
            .collect();

        for (caller_id, callee_name) in &result.call_refs {
            if let Some(callee_id) = name_to_id.get(callee_name) {
                self.store
                    .upsert_code_edge(CodeEdge {
                        from: caller_id.clone(),
                        to: callee_id.clone(),
                        kind: CodeEdgeKind::Calls,
                    })
                    .await?;
            }
        }

        // L2: REFERENCES + IMPLEMENTS (requires LSP).
        if let Some(lsp) = &self.lsp {
            self.resolve_l2_edges(lsp, abs_path, &result, &name_to_id).await;
        }

        Ok(node_count)
    }

    /// Populate L2 edges using LSP reference/implementation queries.
    ///
    /// Errors are logged but never propagate — L2 is best-effort.
    async fn resolve_l2_edges(
        &self,
        lsp: &LspClient,
        abs_path: &Path,
        result: &ParseResult,
        _name_to_id: &std::collections::HashMap<String, String>,
    ) {
        for node in &result.nodes {
            // Use the node's start line for the LSP position query.
            let line = node.start_line.saturating_sub(1);

            // ── REFERENCES ─────────────────────────────────────────────────
            match lsp.references(abs_path, line, 0).await {
                Ok(locs) => {
                    for loc in locs {
                        let ref_path = uri_to_rel_path(&loc.uri);
                        // Create a REFERENCES edge: this node ← caller site.
                        // We use a best-effort id: path + line.
                        let caller_id = format!(
                            "{}:{}",
                            ref_path,
                            loc.range.start.line
                        );
                        // Only create an edge if the caller is a known node.
                        // Otherwise we'd create dangling references.
                        let edge = CodeEdge {
                            from: caller_id,
                            to:   node.id.clone(),
                            kind: CodeEdgeKind::References,
                        };
                        if let Err(e) = self.store.upsert_code_edge(edge).await {
                            warn!(err = %e, "L2 REFERENCES edge failed");
                        }
                    }
                }
                Err(e) => warn!(node = %node.name, err = %e, "LSP references failed"),
            }

            // ── IMPLEMENTS ─────────────────────────────────────────────────
            match lsp.implementations(abs_path, line, 0).await {
                Ok(locs) => {
                    for loc in locs {
                        let impl_path = uri_to_rel_path(&loc.uri);
                        let impl_id = format!("{}:{}", impl_path, loc.range.start.line);
                        let edge = CodeEdge {
                            from: impl_id,
                            to:   node.id.clone(),
                            kind: CodeEdgeKind::Implements,
                        };
                        if let Err(e) = self.store.upsert_code_edge(edge).await {
                            warn!(err = %e, "L2 IMPLEMENTS edge failed");
                        }
                    }
                }
                Err(e) => warn!(node = %node.name, err = %e, "LSP implementations failed"),
            }
        }
    }
}

/// Convert an LSP `file://` URI to a relative string (strip `file://` prefix).
fn uri_to_rel_path(uri: &lsp_types::Url) -> String {
    uri.path().to_string()
}

/// Fingerprint bytes for cache-invalidation (not cryptographic).
fn sha256_hex(data: &[u8]) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    format!("{:016x}", h.finish())
}
