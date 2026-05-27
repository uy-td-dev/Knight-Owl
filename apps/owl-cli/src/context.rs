//! `GraphContextProvider` — thin wrapper that hands every Plan-phase prompt
//! to `owl_vault::hybrid_retrieve` (WF-13, R-20) and, when configured, to
//! cross-session FTS recall (Phase G).
//!
//! The actual retrieval pipelines live in `owl-vault` so the CLI and
//! desktop apps share one implementation.

use std::sync::Arc;

use async_trait::async_trait;

use owl_brain::{BrainError, ContextProvider};
use owl_protocol::memory::MemoryStore;
use owl_vault::{hybrid_retrieve, Embedder, HybridRetrieveConfig, HybridStore};

/// Bridge: hooks WF-13 hybrid retrieval into the reasoning loop, plus
/// optional cross-session memory recall.
pub struct GraphContextProvider {
    store:           Arc<dyn HybridStore>,
    embedder:        Option<Arc<dyn Embedder>>,
    config:          HybridRetrieveConfig,
    /// Memory store used for cross-session FTS recall.  When set, every
    /// retrieve() also pulls top-N hits from OTHER sessions and appends
    /// them under `<past_sessions>`.
    session_memory:  Option<Arc<dyn MemoryStore>>,
    /// Session to exclude from cross-session hits (the active one).
    current_session: String,
    /// Cap on past-session hits per retrieve.
    session_recall_k: usize,
}

impl GraphContextProvider {
    /// Create a BM25-only provider — adequate for read-only navigation but
    /// strictly worse than wiring an embedder.
    pub fn new(store: Arc<dyn HybridStore>) -> Self {
        Self {
            store, embedder: None, config: HybridRetrieveConfig::default(),
            session_memory: None,
            current_session: String::new(),
            session_recall_k: 5,
        }
    }

    /// Attach an embedder to enable the semantic-vector seed leg.
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Enable Phase G cross-session memory recall.  `current_session`
    /// excludes the active session from results so the agent doesn't see
    /// its own in-progress turns echoed back.
    pub fn with_session_recall(
        mut self,
        memory:          Arc<dyn MemoryStore>,
        current_session: String,
    ) -> Self {
        self.session_memory  = Some(memory);
        self.current_session = current_session;
        self
    }
}

#[async_trait]
impl ContextProvider for GraphContextProvider {
    async fn retrieve(&self, prompt: &str) -> Result<Vec<String>, BrainError> {
        // Code graph context — existing.
        let mut snippets = hybrid_retrieve(
            &*self.store,
            self.embedder.as_deref(),
            prompt,
            &self.config,
        )
        .await
        .map_err(|e| BrainError::ContextRetrieval(e.to_string()))?;

        // Phase G — cross-session conversation recall.
        if let Some(mem) = &self.session_memory {
            let hits = mem
                .search_session_memory(
                    prompt,
                    &self.current_session,
                    self.session_recall_k,
                )
                .await
                .map_err(|e| BrainError::ContextRetrieval(e.to_string()))?;
            if !hits.is_empty() {
                let body = hits
                    .iter()
                    .map(|h| format!(
                        "[session:{} role:{}] {}",
                        truncate(&h.session_id, 8),
                        h.role,
                        truncate(&h.content, 200),
                    ))
                    .collect::<Vec<_>>()
                    .join("\n");
                snippets.push(format!(
                    "<past_sessions>\n{body}\n</past_sessions>"
                ));
            }
        }

        Ok(snippets)
    }
}

/// Truncate to `max` chars, appending `…` when cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}
