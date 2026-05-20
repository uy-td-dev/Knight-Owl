//! High-level facade tying extraction and hybrid persistence together.

use std::sync::Arc;

use owl_vault::{Embedder, HybridStore};

use crate::extract::extract;
use crate::query::{ContextChunk, QueryEngine};
use crate::CartographerError;

/// End-to-end GraphRAG indexer + querier.
///
/// Generic over the `rig` completion model used for extraction. Embedder and
/// hybrid store are injected as trait objects (R-5).
pub struct Cartographer<M>
where
    M: rig::completion::CompletionModel + Clone + 'static,
{
    extractor_model: M,
    embedder: Arc<dyn Embedder>,
    store: Arc<dyn HybridStore>,
}

impl<M> Cartographer<M>
where
    M: rig::completion::CompletionModel + Clone + Send + Sync + 'static,
{
    /// Construct a new cartographer.
    pub fn new(
        extractor_model: M,
        embedder: Arc<dyn Embedder>,
        store: Arc<dyn HybridStore>,
    ) -> Self {
        Self { extractor_model, embedder, store }
    }

    /// Ingest a chunk of text: extract entities/relations, embed each entity
    /// description, and persist into the hybrid store.
    pub async fn ingest(&self, chunk: &str) -> Result<usize, CartographerError> {
        let result = extract(self.extractor_model.clone(), chunk).await?;
        let entity_count = result.entities.len();

        for entity in result.entities {
            let embedding = self.embedder.embed(&entity.description).await?;
            self.store.upsert_entity(entity, Some(embedding)).await?;
        }
        for relation in result.relations {
            self.store.upsert_relation(relation).await?;
        }

        Ok(entity_count)
    }

    /// Query the indexed knowledge.
    pub async fn ask(
        &self,
        prompt: &str,
        top_k: u64,
        depth: usize,
    ) -> Result<Vec<ContextChunk>, CartographerError> {
        crate::query::query(&*self.embedder, &*self.store, prompt, top_k, depth).await
    }

    /// Build a re-usable query engine bound to the same stores.
    pub fn query_engine(&self) -> QueryEngine {
        QueryEngine {
            embedder: self.embedder.clone(),
            store: self.store.clone(),
        }
    }
}
