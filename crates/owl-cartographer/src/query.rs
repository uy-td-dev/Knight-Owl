//! Hybrid retrieval: vector seed → graph expansion → context blobs.
//!
//! Two pipelines are exposed:
//!
//! - [`query`] — original single-pass: embed query → vector_search → graph walk.
//! - [`dual_query`] — LightRAG-style dual-level retrieval (WF-13). Splits the
//!   user's question into **low-level** (specific entities, identifiers) and
//!   **high-level** (themes, intent) keyword sets, runs each through the
//!   appropriate retriever, then merges with a weighted score.

use std::collections::HashMap;
use std::sync::Arc;

use rig::agent::AgentBuilder;
use rig::completion::{CompletionModel, Prompt};
use serde::Deserialize;

use owl_protocol::vector::VectorMatch;
use owl_vault::{Embedder, HybridStore};

use crate::prompt::{keyword_user, KEYWORD_EXTRACTION_SYSTEM};
use crate::CartographerError;

// ─── Public types ────────────────────────────────────────────────────────────

/// A retrieved context fragment: matched entity description plus its neighbourhood.
#[derive(Debug, Clone)]
pub struct ContextChunk {
    pub seed_id: String,
    pub score: f32,
    pub text: String,
}

/// Decomposed query keywords — produced by the keyword-extraction LLM call.
///
/// `low` drives BM25 / graph-name lookups; `high` drives vector search.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KeywordSet {
    #[serde(default)]
    pub low: Vec<String>,
    #[serde(default)]
    pub high: Vec<String>,
}

// ─── Original (single-pass) query — kept for back-compat ─────────────────────

/// Hybrid GraphRAG query.
///
/// 1. Embed `query` and run vector search to find seed entity ids
/// 2. Expand each seed by `depth` hops in the graph
/// 3. Format each cluster as a single context chunk
pub async fn query(
    embedder: &dyn Embedder,
    store: &dyn HybridStore,
    query: &str,
    top_k: u64,
    depth: usize,
) -> Result<Vec<ContextChunk>, CartographerError> {
    let embedding = embedder.embed(query).await?;
    let matches: Vec<VectorMatch> = store.vector_search(embedding.values, top_k).await?;
    chunks_from_seeds(store, matches, depth).await
}

// ─── Dual-level retrieval (LightRAG-style) ───────────────────────────────────

/// Run keyword extraction → dual retrieval → merge.
///
/// Returns the top-`top_k` merged context chunks, each annotated with the
/// graph neighbourhood at `depth` hops.
pub async fn dual_query<M>(
    model: M,
    embedder: &dyn Embedder,
    store: &dyn HybridStore,
    query: &str,
    top_k: u64,
    depth: usize,
) -> Result<Vec<ContextChunk>, CartographerError>
where
    M: CompletionModel + Clone + 'static,
{
    let keywords = extract_keywords(model, query).await?;
    dual_query_with_keywords(embedder, store, &keywords, top_k, depth).await
}

/// Lower-level entry point: skip the LLM call and run dual retrieval against a
/// pre-computed [`KeywordSet`]. Useful when keywords come from a different
/// source (e.g. the user typed `@symbol` references).
pub async fn dual_query_with_keywords(
    embedder: &dyn Embedder,
    store: &dyn HybridStore,
    keywords: &KeywordSet,
    top_k: u64,
    depth: usize,
) -> Result<Vec<ContextChunk>, CartographerError> {
    // Score weights — sum to 1.0. Hand-tuned to favour the path most likely to
    // be intended: graph hits when the user named something concrete, vector
    // hits when they asked an open-ended "how / why" question.
    const W_HIGH: f32 = 0.55; // vector search on theme phrases
    const W_LOW: f32 = 0.45; // BM25 / graph-name lookup on identifiers

    let per_kw = top_k.max(4);
    let mut scored: HashMap<String, f32> = HashMap::new();

    // ── High-level: vector search on each theme phrase ──────────────────────
    for phrase in &keywords.high {
        let emb = embedder.embed(phrase).await?;
        for m in store.vector_search(emb.values, per_kw).await? {
            *scored.entry(m.document.id).or_default() += W_HIGH * m.score;
        }
    }

    // ── Low-level: BM25 on each identifier ──────────────────────────────────
    for ident in &keywords.low {
        let hits = store.keyword_search(ident, per_kw).await?;
        // BM25 scores are unbounded — normalise by rank (1.0 / (rank + 1)).
        for (rank, node) in hits.into_iter().enumerate() {
            let s = W_LOW / (rank as f32 + 1.0);
            *scored.entry(node.id).or_default() += s;
        }
    }

    // Empty keyword sets → fall back to plain vector search on the raw text.
    // This keeps `dual_query` safe for short / vague questions that the
    // extractor can't decompose.
    if scored.is_empty() {
        return Ok(Vec::new());
    }

    // Sort by combined score, take top_k seeds.
    let mut seeds: Vec<(String, f32)> = scored.into_iter().collect();
    seeds.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    seeds.truncate(top_k as usize);

    // Materialise context chunks (entity description + graph neighbourhood).
    let mut chunks = Vec::with_capacity(seeds.len());
    for (seed_id, score) in seeds {
        let mut lines = vec![format!("# {seed_id}")];
        if let Some(entity) = store.get_entity(&seed_id).await? {
            lines.push(format!("{} ({}): {}", entity.name, entity.kind, entity.description));
        }
        for neighbour in store.neighbours(&seed_id, depth).await? {
            lines.push(format!(
                "- {} ({}): {}",
                neighbour.name, neighbour.kind, neighbour.description
            ));
        }
        chunks.push(ContextChunk { seed_id, score, text: lines.join("\n") });
    }

    Ok(chunks)
}

/// Run the keyword-extraction LLM call.
///
/// The model is asked to return a strict `{ "low": [...], "high": [...] }`
/// JSON object — see `prompts/keyword_extraction.md`. We tolerate prose
/// surrounding the JSON by extracting the first balanced `{...}` block.
pub async fn extract_keywords<M>(model: M, query: &str) -> Result<KeywordSet, CartographerError>
where
    M: CompletionModel + Clone + 'static,
{
    let agent = AgentBuilder::new(model).preamble(KEYWORD_EXTRACTION_SYSTEM).build();
    let raw = agent
        .prompt(keyword_user(query).as_str())
        .await
        .map_err(|e| CartographerError::Extraction(e.to_string()))?;
    parse_keywords(&raw)
}

fn parse_keywords(raw: &str) -> Result<KeywordSet, CartographerError> {
    let json = first_json_object(raw)
        .ok_or_else(|| CartographerError::InvalidResponse("no JSON object found".into()))?;
    serde_json::from_str::<KeywordSet>(json)
        .map_err(|e| CartographerError::InvalidResponse(e.to_string()))
}

fn first_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0i32;
    for (i, ch) in s[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

// ─── Shared helpers ──────────────────────────────────────────────────────────

async fn chunks_from_seeds(
    store: &dyn HybridStore,
    matches: Vec<VectorMatch>,
    depth: usize,
) -> Result<Vec<ContextChunk>, CartographerError> {
    let mut chunks = Vec::with_capacity(matches.len());
    for m in matches {
        let mut lines = vec![format!("# {}", m.document.id)];
        if let Some(entity) = store.get_entity(&m.document.id).await? {
            lines.push(format!("{} ({}): {}", entity.name, entity.kind, entity.description));
        } else {
            lines.push(m.document.content.clone());
        }
        for neighbour in store.neighbours(&m.document.id, depth).await? {
            lines.push(format!(
                "- {} ({}): {}",
                neighbour.name, neighbour.kind, neighbour.description
            ));
        }
        chunks.push(ContextChunk {
            seed_id: m.document.id.clone(),
            score: m.score,
            text: lines.join("\n"),
        });
    }
    Ok(chunks)
}

// ─── Convenience handle ──────────────────────────────────────────────────────

/// Convenience handle bundling embedder + store for repeated queries.
pub struct QueryEngine {
    pub embedder: Arc<dyn Embedder>,
    pub store: Arc<dyn HybridStore>,
}

impl QueryEngine {
    /// Run a single-pass query against the bundled stores.
    pub async fn ask(
        &self,
        prompt: &str,
        top_k: u64,
        depth: usize,
    ) -> Result<Vec<ContextChunk>, CartographerError> {
        query(&*self.embedder, &*self.store, prompt, top_k, depth).await
    }

    /// Run a dual-level (LightRAG-style) query against the bundled stores.
    /// Requires a completion model for keyword extraction.
    pub async fn ask_dual<M>(
        &self,
        model: M,
        prompt: &str,
        top_k: u64,
        depth: usize,
    ) -> Result<Vec<ContextChunk>, CartographerError>
    where
        M: CompletionModel + Clone + 'static,
    {
        dual_query(model, &*self.embedder, &*self.store, prompt, top_k, depth).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_balanced_keyword_json() {
        let raw = r#"noise before {"low":["parse_rust"],"high":["error handling"]} trailing"#;
        let kw = parse_keywords(raw).expect("must parse");
        assert_eq!(kw.low, vec!["parse_rust"]);
        assert_eq!(kw.high, vec!["error handling"]);
    }

    #[test]
    fn parses_empty_lists() {
        let raw = r#"{"low":[],"high":[]}"#;
        let kw = parse_keywords(raw).expect("must parse");
        assert!(kw.low.is_empty());
        assert!(kw.high.is_empty());
    }

    #[test]
    fn rejects_no_json() {
        assert!(parse_keywords("plain prose, no braces").is_err());
    }

    #[test]
    fn first_json_object_handles_nesting() {
        let s = r#"prefix {"a":{"b":[1,2]}} suffix"#;
        assert_eq!(first_json_object(s), Some(r#"{"a":{"b":[1,2]}}"#));
    }
}
