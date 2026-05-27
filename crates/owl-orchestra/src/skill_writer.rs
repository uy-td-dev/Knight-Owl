//! Filesystem-backed [`owl_brain::SkillWriter`].
//!
//! Persists auto-distilled [`SkillDraft`]s as `.knight-owl/skills/<id>.md`
//! files with the TOML frontmatter `Loader::load_skill` already parses —
//! so once written, the next `OrchestraWatcher` reload picks them up and
//! the agent can compose them into its system prompt with no extra wiring.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use owl_brain::skill_writer::{SkillDraft, SkillWriter};
use owl_brain::BrainError;
use tracing::debug;

/// Writes skill drafts to `<skills_dir>/<id>.md`.
///
/// `skills_dir` is typically the workspace's `.knight-owl/skills/` (per
/// [`crate::path::OrchestraRoots::workspace`]).  The directory is created
/// on first write if it doesn't already exist.
pub struct FsSkillWriter {
    skills_dir: PathBuf,
}

impl FsSkillWriter {
    /// Construct a writer rooted at `skills_dir`.  No I/O happens here;
    /// the directory is created lazily on first `write_skill`.
    pub fn new(skills_dir: impl Into<PathBuf>) -> Self {
        Self { skills_dir: skills_dir.into() }
    }

    /// Convenience: locate the workspace's skills dir from a workspace root.
    /// Returns `<workspace>/.knight-owl/skills/` regardless of whether it
    /// exists — call sites should match this on the `OrchestraRoots` they
    /// already resolved.
    pub fn for_workspace(workspace: &Path) -> Self {
        Self::new(workspace.join(".knight-owl").join("skills"))
    }
}

#[async_trait]
impl SkillWriter for FsSkillWriter {
    async fn write_skill(&self, draft: SkillDraft) -> Result<String, BrainError> {
        tokio::fs::create_dir_all(&self.skills_dir).await
            .map_err(|e| BrainError::Memory(format!(
                "create skills dir {}: {e}",
                self.skills_dir.display()
            )))?;

        // De-dupe: append `-N` until we find an unused filename.
        let id = pick_unique_id(&self.skills_dir, &draft.id).await;
        let path = self.skills_dir.join(format!("{id}.md"));

        let contents = render_skill(&id, &draft);
        tokio::fs::write(&path, contents).await
            .map_err(|e| BrainError::Memory(format!(
                "write skill {}: {e}",
                path.display()
            )))?;
        debug!(path = %path.display(), "auto-skill written");
        Ok(id)
    }
}

/// Render a draft as a TOML-frontmatter markdown file matching the
/// schema `Loader::load_skill` expects.
fn render_skill(id: &str, d: &SkillDraft) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str("schema_version = 1\n");
    s.push_str(&format!("id          = {}\n", toml_string(id)));
    s.push_str(&format!("name        = {}\n", toml_string(&d.name)));
    s.push_str(&format!("description = {}\n", toml_string(&d.description)));
    if let Some(t) = &d.trigger {
        s.push_str(&format!("trigger     = {}\n", toml_string(t)));
    }
    if !d.recommended_tools.is_empty() {
        let arr = d.recommended_tools
            .iter()
            .map(|t| toml_string(t))
            .collect::<Vec<_>>()
            .join(", ");
        s.push_str(&format!("recommended_tools = [{arr}]\n"));
    }
    s.push_str("---\n");
    s.push_str(d.body.trim_end());
    s.push('\n');
    s
}

/// Minimal TOML string quoter — handles backslash + double-quote.  Avoids
/// pulling a TOML serializer for a 6-line escape table.
fn toml_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Probe `<dir>/<id>.md`, `<dir>/<id>-1.md`, … until we find one that
/// doesn't exist.  Bounded at 1000 so a runaway loop can't spin.
async fn pick_unique_id(dir: &Path, base: &str) -> String {
    for i in 0..1000 {
        let candidate = if i == 0 { base.to_string() } else { format!("{base}-{i}") };
        let probe = dir.join(format!("{candidate}.md"));
        if !probe.exists() {
            return candidate;
        }
    }
    // Pathological: just append a random uuid suffix.
    format!("{base}-{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Allocate a unique temp directory and clean it up on drop.
    /// Avoids pulling `tempfile` for a single test module.
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new() -> Self {
            let p = std::env::temp_dir()
                .join(format!("owl-orchestra-test-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path { &self.0 }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[tokio::test]
    async fn writes_skill_to_disk_with_frontmatter() {
        let dir = TmpDir::new();
        let w   = FsSkillWriter::new(dir.path());

        let draft = SkillDraft {
            id:                "auto-test".into(),
            name:              "Test Skill".into(),
            description:       "for the test".into(),
            trigger:           Some("when testing".into()),
            recommended_tools: vec!["bash".into(), "read_file".into()],
            body:              "## When\nalways\n\n## Steps\n1. do the thing".into(),
        };
        let id = w.write_skill(draft).await.unwrap();
        assert_eq!(id, "auto-test");

        let written = std::fs::read_to_string(dir.path().join("auto-test.md")).unwrap();
        assert!(written.starts_with("---\n"));
        assert!(written.contains("schema_version = 1"));
        assert!(written.contains("id          = \"auto-test\""));
        assert!(written.contains("name        = \"Test Skill\""));
        assert!(written.contains("trigger     = \"when testing\""));
        assert!(written.contains("recommended_tools = [\"bash\", \"read_file\"]"));
        assert!(written.contains("## Steps"));
    }

    #[tokio::test]
    async fn dedupe_appends_numeric_suffix() {
        let dir = TmpDir::new();
        let w   = FsSkillWriter::new(dir.path());

        let id1 = w.write_skill(draft("dup")).await.unwrap();
        assert_eq!(id1, "dup");
        let id2 = w.write_skill(draft("dup")).await.unwrap();
        assert_eq!(id2, "dup-1");
    }

    fn draft(id: &str) -> SkillDraft {
        SkillDraft {
            id:                id.into(),
            name:              id.into(),
            description:       "x".into(),
            trigger:           None,
            recommended_tools: vec![],
            body:              "## When\nx".into(),
        }
    }
}
