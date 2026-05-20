//! Runtime configuration for owl-vault.
//!
//! Source priority (low → high): hardcoded defaults → `config/owl-vault.toml` → env vars.

use serde::Deserialize;

use crate::surreal::SurrealConfig;

/// Configuration for the vault store.
///
/// Loaded from `config/owl-vault.toml` if present; fields can be overridden
/// with the `OWL_VAULT_*` env vars documented below.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// SurrealDB endpoint (`mem://`, `rocksdb://…`, or `ws://…`).
    /// Env: `OWL_VAULT_ENDPOINT`
    #[serde(default = "defaults::endpoint")]
    pub endpoint: String,

    /// SurrealDB namespace.
    /// Env: `OWL_VAULT_NAMESPACE`
    #[serde(default = "defaults::namespace")]
    pub namespace: String,

    /// SurrealDB database name.
    /// Env: `OWL_VAULT_DATABASE`
    #[serde(default = "defaults::database")]
    pub database: String,

    /// Optional root user (sent to remote servers that require auth).
    /// Env: `OWL_VAULT_USER`
    #[serde(skip)]
    pub username: Option<String>,

    /// Optional root password.
    /// Env: `OWL_VAULT_PASS`
    #[serde(skip)]
    pub password: Option<String>,
}

impl Config {
    /// Load defaults → toml → env overrides.
    pub fn load() -> Self {
        let path = "config/owl-vault.toml";
        let mut cfg: Config = if std::path::Path::new(path).exists() {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|raw| toml::from_str(&raw).ok())
                .unwrap_or_default()
        } else {
            Config::default()
        };

        if let Ok(v) = std::env::var("OWL_VAULT_ENDPOINT")  { cfg.endpoint  = v; }
        if let Ok(v) = std::env::var("OWL_VAULT_NAMESPACE") { cfg.namespace = v; }
        if let Ok(v) = std::env::var("OWL_VAULT_DATABASE")  { cfg.database  = v; }
        cfg.username = std::env::var("OWL_VAULT_USER").ok();
        cfg.password = std::env::var("OWL_VAULT_PASS").ok();

        // Auto-default to root/root for ws:// or http:// endpoints if user
        // didn't override — SurrealDB Docker images require auth out of the box.
        let needs_auth = cfg.endpoint.starts_with("ws") || cfg.endpoint.starts_with("http");
        if needs_auth && cfg.username.is_none() {
            cfg.username = Some("root".into());
            cfg.password = Some("root".into());
        }
        cfg
    }

    /// Convert into a [`SurrealConfig`] for store construction.
    pub fn into_surreal(self) -> SurrealConfig {
        SurrealConfig {
            endpoint:  self.endpoint,
            namespace: self.namespace,
            database:  self.database,
            username:  self.username,
            password:  self.password,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            endpoint:  defaults::endpoint(),
            namespace: defaults::namespace(),
            database:  defaults::database(),
            username:  None,
            password:  None,
        }
    }
}

mod defaults {
    pub fn endpoint()  -> String { "ws://localhost:8000".into() }
    pub fn namespace() -> String { "knight_owl".into() }
    pub fn database()  -> String { "vault".into() }
}
