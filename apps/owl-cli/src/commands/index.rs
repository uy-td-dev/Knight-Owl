//! `owl index` — walk a workspace and ingest all supported source files into
//! SurrealDB so the reasoning loop can query the code graph.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Args;
use tracing::info;

use owl_cortex::config::Config as CortexConfig;
use owl_cortex::Ingestor;
use owl_vault::surreal::{SurrealConfig, SurrealStore};

/// Index a workspace into the code graph (L1 Syntax layer).
///
/// Reads `OWL_VAULT_ENDPOINT` (default: `ws://localhost:8000`) for the
/// SurrealDB connection.
#[derive(Args)]
pub struct IndexCommand {
    /// Path to the workspace root to index.
    #[arg(default_value = ".")]
    pub workspace: PathBuf,
}

impl IndexCommand {
    pub async fn run(self) -> Result<()> {
        let workspace = self.workspace.canonicalize().context("workspace path")?;

        let endpoint = std::env::var("OWL_VAULT_ENDPOINT")
            .unwrap_or_else(|_| "ws://localhost:8000".into());

        let store = SurrealStore::connect(SurrealConfig {
            endpoint,
            namespace: "knight_owl".into(),
            database: "vault".into(),
            username: std::env::var("OWL_VAULT_USERNAME").ok(),
            password: std::env::var("OWL_VAULT_PASSWORD").ok(),
        })
        .await
        .context("connect to SurrealDB")?;
        let store: Arc<_> = Arc::new(store);

        let ingestor = Ingestor::new(store, CortexConfig::default());

        let mut total = 0usize;
        let mut files = 0usize;

        for entry in walkdir::WalkDir::new(&workspace)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "rs" | "ts" | "tsx" | "js" | "jsx" | "py") {
                continue;
            }
            match ingestor.on_file_changed(path, &workspace).await {
                Ok(n) if n > 0 => {
                    total += n;
                    files += 1;
                    info!(path = %path.display(), nodes = n, "indexed");
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), err = %e, "skipped");
                }
            }
        }

        println!("Indexed {files} files, {total} code nodes.");
        Ok(())
    }
}
