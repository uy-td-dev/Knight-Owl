//! Embedding abstraction — wraps any `rig::embeddings::EmbeddingModel`.

use async_trait::async_trait;

use owl_protocol::vector::Embedding;

use crate::VaultError;

/// Pluggable embedder used by stores and indexers.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Embed a single text into a dense vector.
    async fn embed(&self, text: &str) -> Result<Embedding, VaultError>;

    /// Embed a batch of texts. Default impl loops over [`embed`].
    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Embedding>, VaultError> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(self.embed(&t).await?);
        }
        Ok(out)
    }

    /// Dimensionality of vectors produced by this embedder.
    fn dim(&self) -> usize;
}

/// Adapter: wraps any `rig::embeddings::EmbeddingModel` as an [`Embedder`].
pub struct RigEmbedder<M: rig::embeddings::EmbeddingModel> {
    model: M,
}

impl<M: rig::embeddings::EmbeddingModel> RigEmbedder<M> {
    /// Construct a new embedder from a rig embedding model.
    pub fn new(model: M) -> Self {
        Self { model }
    }
}

#[async_trait]
impl<M> Embedder for RigEmbedder<M>
where
    M: rig::embeddings::EmbeddingModel + Send + Sync,
{
    async fn embed(&self, text: &str) -> Result<Embedding, VaultError> {
        let embeddings = self
            .model
            .embed_texts(vec![text.to_string()])
            .await
            .map_err(|e| VaultError::Embedding(e.to_string()))?;
        let first = embeddings
            .into_iter()
            .next()
            .ok_or_else(|| VaultError::Embedding("empty embedding response".into()))?;
        Ok(Embedding {
            values: first.vec.into_iter().map(|v| v as f32).collect(),
        })
    }

    fn dim(&self) -> usize {
        self.model.ndims()
    }
}

/// Local hash-based embedder — no external API required.
///
/// Uses the feature-hashing trick: each whitespace-delimited token is mapped
/// to a bucket in a fixed-width vector via FNV-1a, then the vector is
/// L2-normalised.  Quality is sufficient for code-keyword recall without any
/// network dependency.  Dimensionality is set at construction time (default
/// [`HashEmbedder::DEFAULT_DIM`]).
pub struct HashEmbedder {
    dim: usize,
}

impl HashEmbedder {
    /// Default vector width — 256 dimensions balances quality and storage.
    pub const DEFAULT_DIM: usize = 256;

    /// Construct a `HashEmbedder` with `dim` dimensions.
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl Default for HashEmbedder {
    fn default() -> Self {
        Self::new(Self::DEFAULT_DIM)
    }
}

#[async_trait]
impl Embedder for HashEmbedder {
    async fn embed(&self, text: &str) -> Result<Embedding, VaultError> {
        let mut vec = vec![0.0f32; self.dim];
        for token in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if token.is_empty() {
                continue;
            }
            let h = fnv1a(token.as_bytes()) as usize % self.dim;
            vec[h] += 1.0;
        }
        // L2-normalise so cosine similarity is well-defined.
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut vec {
                *x /= norm;
            }
        }
        Ok(Embedding { values: vec })
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

/// FNV-1a 64-bit hash — no external crate needed.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 14_695_981_039_346_656_037;
    const PRIME: u64 = 1_099_511_628_211;
    bytes.iter().fold(OFFSET, |h, &b| (h ^ b as u64).wrapping_mul(PRIME))
}
