//! Filesystem watcher — keeps a [`crate::registry::InMemoryRegistry`] in sync
//! with the on-disk `.knight-owl/` tree.
//!
//! Strategy:
//! 1. `notify::recommended_watcher` watches every existing root recursively.
//! 2. Raw events are forwarded over a `tokio::sync::mpsc` channel.
//! 3. A debounce task (200 ms) coalesces bursts (editors save → multiple
//!    events for one logical change), then triggers a **full re-scan**.
//!
//! Why full re-scan instead of incremental updates?
//! - Editors do atomic writes (`tmp → rename`) which generate confusing
//!   events; incremental code drowns in edge cases.
//! - The fixture tree is small (10s, max 100s of files); a re-scan is cheap.
//! - Registry mutations are atomic (`Arc` swap) so in-flight executions
//!   keep their old spec — no torn reads.
//! - One code path (scan) instead of two (scan + apply-event-stream).
//!
//! Trade-off: edits to a single file trigger a re-load of every spec in the
//! root.  At v1 scales (< 100 specs per workspace) this is < 50 ms.  If we
//! ever outgrow that, swap in `notify-debouncer-full` and selective reload.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::loader::Loader;
use crate::path::OrchestraRoots;
use crate::registry::InMemoryRegistry;
use crate::scan::scan_roots;

/// Active filesystem watcher.  Drop the value to stop watching.
pub struct OrchestraWatcher {
    /// Native fs watcher — kept alive so callbacks keep firing.
    _native:  RecommendedWatcher,
    /// Debouncer task; cancelled on drop via the channel close.
    handle:   JoinHandle<()>,
    /// Roots being observed (for diagnostics).
    pub roots: OrchestraRoots,
}

impl OrchestraWatcher {
    /// Spawn a watcher.  Performs an initial scan synchronously before
    /// returning so the registry is hot the moment the function returns.
    pub async fn spawn(
        roots:    OrchestraRoots,
        loader:   Arc<dyn Loader>,
        registry: Arc<InMemoryRegistry>,
    ) -> Result<Self, WatchError> {
        // Initial population.
        let report = scan_roots(&roots, &*loader, &registry).await;
        info!(
            agents = report.agents, skills = report.skills,
            workflows = report.workflows, commands = report.commands,
            errors = report.errors.len(),
            "orchestra initial scan complete",
        );
        for (p, e) in &report.errors {
            warn!(path = ?p, error = %e, "spec load error during initial scan");
        }

        let (tx, rx) = mpsc::unbounded_channel::<Event>();

        // notify spawns its own thread; the closure must be Send + 'static.
        let mut native = notify::recommended_watcher(move |res: notify::Result<Event>| {
            match res {
                Ok(evt)  => { let _ = tx.send(evt); }
                Err(err) => error!(%err, "notify event error"),
            }
        }).map_err(WatchError::Notify)?;

        for root in roots.iter() {
            native.watch(root, RecursiveMode::Recursive)
                .map_err(WatchError::Notify)?;
            debug!(root = ?root, "watching root");
        }

        let roots_clone    = roots.clone();
        let loader_clone   = Arc::clone(&loader);
        let registry_clone = Arc::clone(&registry);
        let handle = tokio::spawn(debounce_loop(
            rx, roots_clone, loader_clone, registry_clone,
        ));

        Ok(Self { _native: native, handle, roots })
    }

    /// Wait for the debounce task to terminate.  Used by tests; production
    /// code typically just lets the value drop with the application.
    pub async fn join(self) {
        let _ = self.handle.await;
    }
}

/// Coalesces bursts of fs events into one re-scan per quiet period.
async fn debounce_loop(
    mut rx:   mpsc::UnboundedReceiver<Event>,
    roots:    OrchestraRoots,
    loader:   Arc<dyn Loader>,
    registry: Arc<InMemoryRegistry>,
) {
    const DEBOUNCE: Duration = Duration::from_millis(200);

    loop {
        // Block for the next event.
        let first = match rx.recv().await {
            Some(evt) => evt,
            None      => return, // channel closed → watcher dropped
        };
        if !is_relevant(&first) { continue; }

        // Coalesce additional events that arrive within the debounce window.
        let mut last_seen = std::time::Instant::now();
        loop {
            let elapsed = last_seen.elapsed();
            let remain  = DEBOUNCE.saturating_sub(elapsed);
            if remain.is_zero() { break; }

            match tokio::time::timeout(remain, rx.recv()).await {
                Ok(Some(evt)) if is_relevant(&evt) => {
                    last_seen = std::time::Instant::now();
                }
                Ok(Some(_))  => continue,    // ignored kind, keep waiting
                Ok(None)     => return,      // channel closed
                Err(_)       => break,       // timeout → quiet period reached
            }
        }

        // Quiet — do the re-scan.
        let report = scan_roots(&roots, &*loader, &registry).await;
        info!(
            agents = report.agents, skills = report.skills,
            workflows = report.workflows, commands = report.commands,
            errors = report.errors.len(),
            "orchestra reload",
        );
        for (p, e) in &report.errors {
            warn!(path = ?p, error = %e, "spec reload error");
        }
    }
}

/// Filter raw fs events down to "things we care about".  Anything except
/// access events triggers a reload — create / write / remove / rename.
fn is_relevant(evt: &Event) -> bool {
    matches!(
        evt.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

// ─── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("filesystem watcher init failed: {0}")]
    Notify(#[source] notify::Error),
    #[error("watched root not found: {0}")]
    RootMissing(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::FsLoader;
    use crate::registry::Registry;
    use std::time::Duration;

    /// Smoke: spawn watcher → modify a file → registry sees the new value.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn picks_up_workflow_edit() {
        // Use a temp dir so we don't touch the real fixture tree.
        let tmp = tempdir();
        let ws  = tmp.join("ws");
        std::fs::create_dir_all(ws.join(".knight-owl/workflows")).unwrap();

        let workflow_path = ws.join(".knight-owl/workflows/demo.toml");
        std::fs::write(&workflow_path, r#"
schema_version = 1

[identity]
id          = "demo"
name        = "Demo"
description = "v1"

[budget]
timeout_ms = 60000
on_failure = "abort"
"#).unwrap();

        let roots    = OrchestraRoots {
            workspace: Some(ws.join(".knight-owl")),
            global:    None,
        };
        let registry = Arc::new(InMemoryRegistry::new());
        let loader: Arc<dyn Loader> = Arc::new(FsLoader);

        let watcher = OrchestraWatcher::spawn(roots, loader, Arc::clone(&registry))
            .await
            .expect("spawn watcher");

        // Initial scan should see version 1.
        let wf = registry.workflow(
            &owl_protocol::orchestra::WorkflowId::new("demo").unwrap()
        ).expect("loaded");
        assert_eq!(wf.description, "v1");

        // Modify file → wait past debounce → check reload.
        std::fs::write(&workflow_path, r#"
schema_version = 1

[identity]
id          = "demo"
name        = "Demo"
description = "v2"

[budget]
timeout_ms = 60000
on_failure = "abort"
"#).unwrap();

        // 200ms debounce + filesystem latency tolerance.
        tokio::time::sleep(Duration::from_millis(800)).await;

        let wf2 = registry.workflow(
            &owl_protocol::orchestra::WorkflowId::new("demo").unwrap()
        ).expect("still loaded");
        assert_eq!(wf2.description, "v2");

        drop(watcher);
        // tempdir cleanup happens via Drop.
    }

    /// Tiny tempdir helper to avoid pulling in the `tempfile` crate.
    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("owl_watcher_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
