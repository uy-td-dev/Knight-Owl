//! Vector / embedding types shared across crates.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A dense embedding vector.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Embedding {
    /// Raw float components.
    pub values: Vec<f32>,
}

impl Embedding {
    /// Dimensionality of the embedding.
    pub fn dim(&self) -> usize {
        self.values.len()
    }
}

/// A document stored in (or returned from) a vector store.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VectorDocument {
    /// Stable identifier (UUID, hash, or app-defined key).
    pub id: String,
    /// Original textual content.
    pub content: String,
    /// Pre-computed embedding (optional on insert if the store computes it).
    pub embedding: Option<Embedding>,
    /// Free-form metadata payload.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A search result: document plus similarity score.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VectorMatch {
    /// The matching document.
    pub document: VectorDocument,
    /// Similarity score (cosine, dot, etc. — store-defined).
    pub score: f32,
}
