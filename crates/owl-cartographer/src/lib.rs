//! The Cartographer — GraphRAG over the agent's working memory.
//!
//! Pipeline:
//!   text chunk → LLM extract entities + relations → HybridStore (owl-vault)
//!                                                   (persists graph + embeddings in one DB)
//!
//! Query time: hybrid retrieval — vector search seeds entity nodes, graph walk
//! expands neighbours, results returned as context blobs.
//!
//! `owl-cartographer` depends on `owl-protocol`, `owl-vault`, and `rig-core` only.

#![forbid(unsafe_code)]

pub mod cartographer;
pub mod error;
pub mod extract;
pub mod prompt;
pub mod query;

pub use cartographer::Cartographer;
pub use error::CartographerError;
pub use query::{
    dual_query, dual_query_with_keywords, extract_keywords, query, ContextChunk, KeywordSet,
    QueryEngine,
};

/// Trait-object-safe interface for graph-based text ingestion.
///
/// Allows callers (e.g. the index pipeline) to hold `Arc<dyn GraphIngester>`
/// without knowing the concrete `CompletionModel` type parameter.
#[async_trait::async_trait]
pub trait GraphIngester: Send + Sync {
    /// Ingest a text chunk: extract entities/relations, embed, persist.
    /// Returns count of entities extracted.
    async fn ingest(&self, chunk: &str) -> Result<usize, CartographerError>;
}

#[async_trait::async_trait]
impl<M> GraphIngester for Cartographer<M>
where
    M: rig::completion::CompletionModel + Clone + Send + Sync + 'static,
{
    async fn ingest(&self, chunk: &str) -> Result<usize, CartographerError> {
        Cartographer::ingest(self, chunk).await
    }
}
