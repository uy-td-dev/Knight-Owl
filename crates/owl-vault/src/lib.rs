//! The Vault — hybrid graph + vector store for semantic memory.
//!
//! Exposes:
//! - [`embedder::Embedder`] trait + [`embedder::HashEmbedder`] (local, no API)
//! - [`store::HybridStore`] trait — the only API consumers should use
//! - [`surreal::SurrealStore`] — SurrealDB-backed implementation
//! - [`memory::SurrealMemoryStore`] — persistent, optionally semantic memory
//!
//! `owl-vault` depends only on `owl-protocol`, `rig-core`, and `surrealdb` (R-13).

#![forbid(unsafe_code)]

pub mod config;
pub mod embedder;
pub mod error;
pub mod experience;
pub mod memory;
pub mod pet;
pub mod store;
pub mod surreal;

pub use config::Config as VaultConfig;
pub use embedder::{Embedder, HashEmbedder, RigEmbedder};
pub use error::VaultError;
pub use experience::{Distiller, SurrealExperienceStore};
pub use memory::SurrealMemoryStore;
pub use pet::{PetStore, SurrealPetStore};
pub use store::{EntityGraphData, FileWithStats, GraphData, HybridStore, KbStats};
pub use surreal::{SurrealConfig, SurrealStore};
