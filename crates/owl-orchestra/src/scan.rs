//! Recursive scanner — populates a [`Registry`] from a directory tree.
//!
//! Layout assumed (Design Spec §2):
//!
//! ```text
//! <root>/
//! ├── agents/<id>/agent.toml + system.md
//! ├── skills/<id>.md
//! ├── workflows/<id>.toml
//! └── commands/<id>.md
//! ```
//!
//! Scanning is **best-effort**: a bad file is logged and skipped, the rest
//! still load.  This is the right tradeoff for hot-reload — one corrupt edit
//! shouldn't take the whole registry offline.

use std::path::{Path, PathBuf};

use owl_protocol::orchestra::SpecKind;
use tracing::{error, warn};

use crate::error::OrchestraError;
use crate::loader::Loader;
use crate::path::{dirs, OrchestraRoots};
use crate::registry::InMemoryRegistry;

/// Outcome of a single root scan.
#[derive(Debug, Default)]
pub struct ScanReport {
    pub agents:    usize,
    pub skills:    usize,
    pub workflows: usize,
    pub commands:  usize,
    pub errors:    Vec<(PathBuf, OrchestraError)>,
}

impl ScanReport {
    pub fn total(&self) -> usize {
        self.agents + self.skills + self.workflows + self.commands
    }
}

/// Scan every existing root in [`OrchestraRoots`] (workspace first, then
/// global) and register everything found.  Workspace duplicates over global
/// silently — see [`crate::registry::InMemoryRegistry`] duplicate detection.
pub async fn scan_roots(
    roots:    &OrchestraRoots,
    loader:   &dyn Loader,
    registry: &InMemoryRegistry,
) -> ScanReport {
    let mut report = ScanReport::default();
    for root in roots.iter() {
        scan_root(root, loader, registry, &mut report).await;
    }
    report
}

async fn scan_root(
    root:     &Path,
    loader:   &dyn Loader,
    registry: &InMemoryRegistry,
    report:   &mut ScanReport,
) {
    scan_agents(root,   loader, registry, report).await;
    scan_kind(root, dirs::SKILLS,    SpecKind::Skill,    loader, registry, report).await;
    scan_kind(root, dirs::WORKFLOWS, SpecKind::Workflow, loader, registry, report).await;
    scan_kind(root, dirs::COMMANDS,  SpecKind::Command,  loader, registry, report).await;
}

/// Agents have a folder-per-agent layout, not single-file — handle separately.
async fn scan_agents(
    root:     &Path,
    loader:   &dyn Loader,
    registry: &InMemoryRegistry,
    report:   &mut ScanReport,
) {
    let agents_dir = root.join(dirs::AGENTS);
    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(it) => it,
        Err(_) => return,             // dir absent is fine
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() { continue; }
        match loader.load_agent(&path).await {
            Ok(spec) => {
                if let Err(e) = registry.register_agent(spec, path.clone()) {
                    warn!(?path, %e, "register_agent failed");
                    report.errors.push((path, e));
                } else {
                    report.agents += 1;
                }
            }
            Err(e) => {
                error!(?path, %e, "load_agent failed");
                report.errors.push((path, e));
            }
        }
    }
}

async fn scan_kind(
    root:     &Path,
    subdir:   &str,
    kind:     SpecKind,
    loader:   &dyn Loader,
    registry: &InMemoryRegistry,
    report:   &mut ScanReport,
) {
    let dir = root.join(subdir);
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() { continue; }

        // Filter by extension to avoid loading editor backups (.swp, .bak…).
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let want_ext = match kind {
            SpecKind::Skill | SpecKind::Command => "md",
            SpecKind::Workflow                  => "toml",
            SpecKind::Agent                     => unreachable!("agents handled separately"),
        };
        if ext != want_ext { continue; }

        match kind {
            SpecKind::Skill => match loader.load_skill(&path).await {
                Ok(s) => match registry.register_skill(s, path.clone()) {
                    Ok(()) => report.skills += 1,
                    Err(e) => report.errors.push((path, e)),
                },
                Err(e) => report.errors.push((path, e)),
            },
            SpecKind::Workflow => match loader.load_workflow(&path).await {
                Ok(s) => match registry.register_workflow(s, path.clone()) {
                    Ok(()) => report.workflows += 1,
                    Err(e) => report.errors.push((path, e)),
                },
                Err(e) => report.errors.push((path, e)),
            },
            SpecKind::Command => match loader.load_command(&path).await {
                Ok(s) => match registry.register_command(s, path.clone()) {
                    Ok(()) => report.commands += 1,
                    Err(e) => report.errors.push((path, e)),
                },
                Err(e) => report.errors.push((path, e)),
            },
            SpecKind::Agent => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::FsLoader;

    #[tokio::test]
    async fn scans_full_fixture_tree() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let roots = OrchestraRoots {
            workspace: Some(fixtures),
            global:    None,
        };
        let registry = InMemoryRegistry::new();
        let report   = scan_roots(&roots, &FsLoader, &registry).await;
        assert_eq!(report.agents,    1);
        assert_eq!(report.skills,    1);
        assert_eq!(report.workflows, 1);
        assert_eq!(report.commands,  1);
        assert!(report.errors.is_empty(), "unexpected errors: {:?}", report.errors);
    }

    #[tokio::test]
    async fn nonexistent_root_yields_empty_report() {
        let roots = OrchestraRoots {
            workspace: Some(PathBuf::from("/nonexistent/owl/test/abc123")),
            global:    None,
        };
        let registry = InMemoryRegistry::new();
        let report   = scan_roots(&roots, &FsLoader, &registry).await;
        assert_eq!(report.total(), 0);
    }
}
