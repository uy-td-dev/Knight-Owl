//! Knowledge-graph types used by `owl-cartographer` (GraphRAG).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A named entity extracted from source text.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Entity {
    /// Canonical identifier (typically the lowercased name).
    pub id: String,
    /// Display name as it appeared in the source.
    pub name: String,
    /// Entity category, e.g. `"person"`, `"org"`, `"concept"`.
    pub kind: String,
    /// One-sentence description synthesized by the extractor.
    pub description: String,
}

/// A typed relation between two entities.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Relation {
    /// Source entity id.
    pub source: String,
    /// Target entity id.
    pub target: String,
    /// Verb-like relation label, e.g. `"founded"`, `"uses"`.
    pub label: String,
    /// Confidence in `[0.0, 1.0]`.
    pub weight: f32,
}

/// Result of running entity + relation extraction over a chunk of text.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractionResult {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

/// A community / cluster of related entities (used in hierarchical GraphRAG summaries).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Community {
    /// Stable identifier for the community.
    pub id: String,
    /// Member entity ids.
    pub members: Vec<String>,
    /// LLM-synthesized summary of the community.
    pub summary: String,
}
