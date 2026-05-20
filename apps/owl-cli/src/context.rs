//! `GraphContextProvider` — bridges owl-vault's code graph to owl-brain's
//! `ContextProvider` trait.
//!
//! On each planning step the reasoning loop calls `retrieve(prompt)`:
//!   1. Extract keywords from the prompt (split on whitespace + punctuation).
//!   2. For each keyword that is ≥ 4 chars, run `search_code_nodes`.
//!   3. Deduplicate by node id, keep the top 20 by relevance.
//!   4. Format each as a self-contained snippet for the LLM.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use owl_brain::{BrainError, ContextProvider};
use owl_protocol::code::CodeNode;
use owl_vault::HybridStore;

/// Provides code-graph context from the hybrid store.
pub struct GraphContextProvider {
    store: Arc<dyn HybridStore>,
}

impl GraphContextProvider {
    /// Create a provider backed by the given store.
    pub fn new(store: Arc<dyn HybridStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ContextProvider for GraphContextProvider {
    async fn retrieve(&self, prompt: &str) -> Result<Vec<String>, BrainError> {
        let keywords = extract_keywords(prompt);
        if keywords.is_empty() {
            return Ok(Vec::new());
        }

        let mut seen: HashMap<String, CodeNode> = HashMap::new();
        for kw in &keywords {
            let nodes = self
                .store
                .search_code_nodes(kw, 10)
                .await
                .map_err(|e| BrainError::ContextRetrieval(e.to_string()))?;
            for node in nodes {
                seen.entry(node.id.clone()).or_insert(node);
            }
            if seen.len() >= 20 {
                break;
            }
        }

        Ok(seen.into_values().map(format_node).collect())
    }
}

/// Format a code node as a single context snippet for the LLM.
fn format_node(n: CodeNode) -> String {
    let mut s = format!(
        "[{}:L{}-{}] {:?} `{}`",
        n.file_path, n.start_line, n.end_line, n.kind, n.name
    );
    if !n.description.is_empty() {
        s.push_str(&format!("\n  doc: {}", n.description.lines().next().unwrap_or("")));
    }
    if !n.preview.is_empty() {
        s.push_str(&format!("\n  {}", n.preview.trim()));
    }
    s
}

/// Split prompt into keywords ≥ 4 chars, deduplicated.
fn extract_keywords(prompt: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    prompt
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() >= 4)
        .map(|w| w.to_lowercase())
        .filter(|w| seen.insert(w.clone()))
        .collect()
}
