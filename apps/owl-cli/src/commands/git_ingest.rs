//! `owl git-ingest` subcommand.
//!
//! Reads the git log of a workspace and persists commit records into the
//! L4 Experience layer (SurrealDB `commit` table).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Args;
use tracing::{info, warn};

use owl_protocol::git::GitCommit;
use owl_vault::{HybridStore, VaultConfig};

/// Ingest git commit history into the L4 knowledge graph.
#[derive(Args)]
pub struct GitIngestCommand {
    /// Path to the git workspace (must contain a `.git` directory).
    pub workspace: PathBuf,

    /// Maximum number of commits to ingest (most recent first).
    #[arg(long, default_value_t = 500)]
    pub limit: usize,

    /// Output raw JSON summary instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

impl GitIngestCommand {
    pub async fn run(self) -> Result<()> {
        let workspace = self
            .workspace
            .canonicalize()
            .context("workspace path not found")?;

        // Open the git repository (searches upward from `workspace`).
        let repo = git2::Repository::discover(&workspace)
            .with_context(|| format!("no git repository found at {}", workspace.display()))?;

        let commits = collect_commits(&repo, self.limit)?;
        let total = commits.len();
        info!(total, "git log collected");

        // Connect to SurrealDB and persist.
        let cfg = VaultConfig::load();
        let store = owl_vault::surreal::SurrealStore::connect(cfg.into_surreal())
            .await
            .context("SurrealDB unreachable — run with a reachable SurrealDB instance")?;
        let store: Arc<dyn HybridStore> = Arc::new(store);

        let mut ingested = 0usize;
        for commit in &commits {
            match store.store_commit(commit.clone()).await {
                Ok(())  => ingested += 1,
                Err(e)  => warn!(hash = %commit.short_hash, err = %e, "failed to store commit"),
            }
        }

        if self.json {
            println!(
                "{}",
                serde_json::json!({ "total": total, "ingested": ingested })
            );
        } else {
            println!("git-ingest: {ingested}/{total} commits stored");
        }
        Ok(())
    }
}

/// Walk the git log and return up to `limit` commits newest-first.
fn collect_commits(repo: &git2::Repository, limit: usize) -> Result<Vec<GitCommit>> {
    let mut revwalk = repo.revwalk().context("failed to create revwalk")?;
    revwalk.push_head().context("repository has no HEAD")?;
    revwalk.set_sorting(git2::Sort::TIME).context("sort failed")?;

    let mut commits = Vec::with_capacity(limit.min(512));

    for oid in revwalk.take(limit) {
        let oid = oid.context("revwalk error")?;
        let commit = repo.find_commit(oid).context("commit lookup failed")?;

        let hash       = oid.to_string();
        let short_hash = hash[..7].to_string();
        let message    = commit.summary().unwrap_or("").to_string();
        let author     = commit.author();
        let author_name  = author.name().unwrap_or("").to_string();
        let author_email = author.email().unwrap_or("").to_string();
        let timestamp    = commit.time().seconds();

        let (files_changed, insertions, deletions) = diff_stats(repo, &commit);

        commits.push(GitCommit {
            hash,
            short_hash,
            message,
            author_name,
            author_email,
            timestamp,
            files_changed,
            insertions,
            deletions,
        });
    }

    Ok(commits)
}

/// Compute diff stats for a commit against its first parent (or empty tree).
fn diff_stats(
    repo: &git2::Repository,
    commit: &git2::Commit<'_>,
) -> (Vec<String>, u32, u32) {
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => return (vec![], 0, 0),
    };

    let parent_tree = commit
        .parent(0)
        .ok()
        .and_then(|p| p.tree().ok());

    let diff = match repo.diff_tree_to_tree(
        parent_tree.as_ref(),
        Some(&tree),
        None,
    ) {
        Ok(d)  => d,
        Err(_) => return (vec![], 0, 0),
    };

    let stats = diff.stats().unwrap_or_else(|_| {
        // Safety: DiffStats can't really fail here, but fall back gracefully.
        diff.stats().expect("diff stats failed twice")
    });

    let mut files_changed: Vec<String> = Vec::new();
    diff.foreach(
        &mut |delta, _| {
            if let Some(path) = delta.new_file().path() {
                files_changed.push(path.to_string_lossy().into_owned());
            }
            true
        },
        None,
        None,
        None,
    )
    .ok();

    (
        files_changed,
        stats.insertions() as u32,
        stats.deletions() as u32,
    )
}
