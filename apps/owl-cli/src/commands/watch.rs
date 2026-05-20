//! `owl watch` subcommand — continuously re-indexes a workspace on file change.
//!
//! Uses the `notify` crate's native file-system events (FSEvents on macOS,
//! inotify on Linux).  Each changed file is forwarded to
//! [`owl_cortex::Ingestor`]; the ingestor is idempotent, so rapid saves produce
//! no duplicate graph nodes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;
use notify::event::{EventKind, ModifyKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{info, warn};

use owl_cortex::Ingestor;
use owl_vault::VaultConfig;

/// Watch a workspace directory and re-index modified source files in real time.
#[derive(Args)]
pub struct WatchCommand {
    /// Root directory to watch (defaults to current directory).
    #[arg(default_value = ".")]
    pub workspace: PathBuf,

    /// Milliseconds to wait after a change before triggering ingest (debounce).
    #[arg(long, default_value_t = 300)]
    pub debounce_ms: u64,
}

impl WatchCommand {
    pub async fn run(self) -> Result<()> {
        let root = self
            .workspace
            .canonicalize()
            .with_context(|| format!("cannot resolve workspace: {}", self.workspace.display()))?;

        info!(path = %root.display(), "starting file watcher");

        let vault_cfg = VaultConfig::load();
        let store = owl_vault::SurrealStore::connect(vault_cfg.into_surreal())
            .await
            .with_context(|| "SurrealDB unreachable — is `docker compose up` running?")?;
        let store: Arc<dyn owl_vault::HybridStore> = Arc::new(store);
        let cortex_cfg = owl_cortex::Config::load()
            .context("failed to load owl-cortex config")?;
        let ingestor = Ingestor::new(Arc::clone(&store), cortex_cfg);

        let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
        let debounce = Duration::from_millis(self.debounce_ms);

        // Spawn the synchronous notify watcher on a blocking thread.
        let mut watcher: RecommendedWatcher = {
            let tx = tx.clone();
            notify::recommended_watcher(move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    if should_ingest(&event.kind) {
                        for path in event.paths {
                            let _ = tx.send(path);
                        }
                    }
                }
            })
            .context("failed to create file watcher")?
        };

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .with_context(|| format!("cannot watch {}", root.display()))?;

        info!("watching {} — Ctrl-C to stop", root.display());

        // Debounce: collect paths, then flush after `debounce` quiet period.
        let mut pending: Vec<PathBuf> = Vec::new();
        let mut deadline = tokio::time::Instant::now() + debounce;

        loop {
            tokio::select! {
                Some(path) = rx.recv() => {
                    pending.push(path);
                    deadline = tokio::time::Instant::now() + debounce;
                }
                _ = tokio::time::sleep_until(deadline), if !pending.is_empty() => {
                    let batch = std::mem::take(&mut pending);
                    for path in batch {
                        if let Err(e) = ingestor.on_file_changed(&path, &root).await {
                            warn!(path = %path.display(), err = %e, "ingest error");
                        } else {
                            info!(path = %path.display(), "re-indexed");
                        }
                    }
                }
            }
        }
    }
}

/// Only ingest on write/create/rename events — ignore metadata and access events.
fn should_ingest(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Name(_))
    )
}
