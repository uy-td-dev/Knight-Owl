//! `PetStore` — SurrealDB-backed persistence for the Owl chibi pet.
//!
//! Single row per workspace, identified by a stable `pet_id` string the host
//! derives from the workspace path.  The table is schema-less; we round-trip
//! through `serde_json::Value` so future fields land without migrations.

use async_trait::async_trait;
use serde_json::{json, Value};
use surrealdb::engine::any::{connect, Any};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;

use owl_protocol::pet::Pet;

use crate::surreal::SurrealConfig;
use crate::VaultError;

/// Async pet load/save API.  Use [`SurrealPetStore`] in production;
/// hosts that want an in-memory fallback can implement this trait too.
#[async_trait]
pub trait PetStore: Send + Sync {
    /// Load the pet for `id`.  Returns `None` if no row exists yet.
    async fn load(&self, id: &str) -> Result<Option<Pet>, VaultError>;

    /// Upsert the pet for `id`.  Always succeeds — creates the row if
    /// missing, overwrites otherwise.
    async fn save(&self, id: &str, pet: &Pet) -> Result<(), VaultError>;
}

/// SurrealDB-backed pet store.  Reuses the same `vault` database as
/// memory / experience / knowledge graph.
pub struct SurrealPetStore {
    db: Surreal<Any>,
}

impl SurrealPetStore {
    /// Connect, sign in, and ensure the `pet` table exists.
    pub async fn connect(cfg: SurrealConfig) -> Result<Self, VaultError> {
        let db = connect(cfg.endpoint).await
            .map_err(|e: surrealdb::Error| VaultError::Surreal(e.to_string()))?;

        if let (Some(u), Some(p)) = (cfg.username.as_ref(), cfg.password.as_ref()) {
            db.signin(Root { username: u.clone(), password: p.clone() }).await
                .map_err(|e: surrealdb::Error| VaultError::Surreal(format!("signin: {e}")))?;
        }
        db.use_ns(cfg.namespace).use_db(cfg.database).await
            .map_err(|e: surrealdb::Error| VaultError::Surreal(e.to_string()))?;
        // Schema-less table is created lazily on first UPSERT, but DEFINE
        // makes failures (e.g. permission) surface immediately at boot.
        db.query("DEFINE TABLE IF NOT EXISTS pet SCHEMALESS").await
            .map_err(|e: surrealdb::Error| VaultError::Surreal(e.to_string()))?;
        Ok(Self { db })
    }
}

#[async_trait]
impl PetStore for SurrealPetStore {
    async fn load(&self, id: &str) -> Result<Option<Pet>, VaultError> {
        let mut resp = self
            .db
            .query("SELECT name, level, xp, hunger, happiness, energy, \
                           last_fed_ms, last_active_ms, tools_witnessed, last_say \
                    FROM type::record('pet', $id) LIMIT 1")
            .bind(json!({ "id": id }))
            .await
            .map_err(|e: surrealdb::Error| VaultError::Surreal(e.to_string()))?
            .check()
            .map_err(|e: surrealdb::Error| VaultError::Surreal(e.to_string()))?;
        let rows: Vec<Value> = resp.take(0)
            .map_err(|e: surrealdb::Error| VaultError::Surreal(e.to_string()))?;
        if let Some(v) = rows.into_iter().next() {
            let pet: Pet = serde_json::from_value(v)
                .map_err(|e| VaultError::Surreal(format!("decode pet: {e}")))?;
            Ok(Some(pet))
        } else {
            Ok(None)
        }
    }

    async fn save(&self, id: &str, pet: &Pet) -> Result<(), VaultError> {
        let payload = serde_json::to_value(pet)
            .map_err(|e| VaultError::Surreal(format!("encode pet: {e}")))?;
        self.db
            .query("UPSERT type::record('pet', $id) CONTENT $payload")
            .bind(json!({ "id": id, "payload": payload }))
            .await
            .map_err(|e: surrealdb::Error| VaultError::Surreal(e.to_string()))?
            .check()
            .map_err(|e: surrealdb::Error| VaultError::Surreal(e.to_string()))?;
        Ok(())
    }
}
