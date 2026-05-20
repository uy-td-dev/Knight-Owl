//! L4 Git history types — commits and pull requests persisted in the knowledge graph.

use serde::{Deserialize, Serialize};

/// A git commit record (L4 Experience layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommit {
    /// Full 40-character SHA hash.
    pub hash: String,
    /// Short (7-char) hash for display.
    pub short_hash: String,
    /// Commit message subject line.
    pub message: String,
    /// Author display name.
    pub author_name: String,
    /// Author email address.
    pub author_email: String,
    /// Unix timestamp (seconds since epoch).
    pub timestamp: i64,
    /// Relative paths of files changed in this commit.
    pub files_changed: Vec<String>,
    /// Number of lines added.
    pub insertions: u32,
    /// Number of lines deleted.
    pub deletions: u32,
}

/// A pull-request record (L4 Experience layer).
///
/// Populated by `owl git-ingest` from GitHub/GitLab API or local PR metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitPullRequest {
    /// Platform-assigned PR number.
    pub number: u64,
    /// PR title.
    pub title: String,
    /// PR body / description.
    pub body: String,
    /// Author login.
    pub author: String,
    /// Current state: `"open"`, `"closed"`, `"merged"`.
    pub state: String,
    /// Merge commit hash (if merged).
    pub merge_commit: Option<String>,
    /// Hashes of commits included in this PR.
    pub commits: Vec<String>,
    /// Unix timestamp the PR was created.
    pub created_at: i64,
    /// Unix timestamp the PR was closed/merged (None if still open).
    pub closed_at: Option<i64>,
}
