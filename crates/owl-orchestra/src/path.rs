//! Path resolution for orchestra spec roots.
//!
//! Two locations are searched, in order:
//!
//! 1. **Workspace** — `<workspace>/.knight-owl/` (project-scoped, version-controlled)
//! 2. **Global**    — `~/.knight-owl/` (per-user fallback)
//!
//! Workspace **overrides** global; we never merge.  A loader walks both
//! roots; the registry rejects duplicate ids with [`crate::OrchestraError::DuplicateId`].

use std::path::{Path, PathBuf};

/// Roots discovered for a given workspace.
#[derive(Debug, Clone)]
pub struct OrchestraRoots {
    /// `<workspace>/.knight-owl/` if the directory exists, else `None`.
    pub workspace: Option<PathBuf>,
    /// `~/.knight-owl/` if `HOME` is set and the directory exists, else `None`.
    pub global:    Option<PathBuf>,
}

impl OrchestraRoots {
    /// Discover both roots without creating them.
    pub fn discover(workspace: &Path) -> Self {
        let ws = {
            let p = workspace.join(".knight-owl");
            if p.is_dir() { Some(p) } else { None }
        };
        let global = match std::env::var_os("HOME") {
            Some(home) => {
                let p = PathBuf::from(home).join(".knight-owl");
                if p.is_dir() { Some(p) } else { None }
            }
            None => None,
        };
        Self { workspace: ws, global }
    }

    /// Returns each existing root in priority order (workspace before global).
    pub fn iter(&self) -> impl Iterator<Item = &Path> {
        self.workspace.as_deref().into_iter().chain(self.global.as_deref())
    }
}

/// Sub-folder names under each root, one per [`SpecKind`].
pub mod dirs {
    pub const AGENTS:    &str = "agents";
    pub const SKILLS:    &str = "skills";
    pub const WORKFLOWS: &str = "workflows";
    pub const COMMANDS:  &str = "commands";
}

/// File-name conventions inside an agent directory.
pub mod agent_files {
    pub const AGENT_TOML: &str = "agent.toml";
    pub const SYSTEM_MD:  &str = "system.md";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_returns_none_for_nonexistent_workspace() {
        let tmp = std::env::temp_dir().join("owl_orchestra_test_nonexistent_xyz123");
        let roots = OrchestraRoots::discover(&tmp);
        assert!(roots.workspace.is_none());
    }

    #[test]
    fn iter_yields_workspace_before_global() {
        let r = OrchestraRoots {
            workspace: Some(PathBuf::from("/ws")),
            global:    Some(PathBuf::from("/global")),
        };
        let v: Vec<_> = r.iter().collect();
        assert_eq!(v[0], Path::new("/ws"));
        assert_eq!(v[1], Path::new("/global"));
    }
}
