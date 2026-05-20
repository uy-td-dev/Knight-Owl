//! Errors for orchestra loader / registry / engine.
//!
//! Spec-validation errors (id pattern, schema version mismatch, unresolved
//! reference) are re-exported from [`owl_protocol::orchestra::OrchestraProtoError`];
//! everything I/O-flavoured is defined here.

use std::path::PathBuf;

use owl_protocol::orchestra::{OrchestraProtoError, SpecKind};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrchestraError {
    #[error("io error reading {path}: {source}")]
    Io { path: PathBuf, #[source] source: std::io::Error },

    #[error("toml parse error in {path}: {source}")]
    Toml { path: PathBuf, #[source] source: toml::de::Error },

    #[error("frontmatter error in {path}: {kind}")]
    Frontmatter { path: PathBuf, kind: FrontmatterErrorKind },

    #[error("missing required file `{file}` in agent directory `{agent_dir}`")]
    MissingFile { agent_dir: PathBuf, file: &'static str },

    #[error("folder name `{folder}` does not match identity.id `{declared}` in {path}")]
    IdMismatch { folder: String, declared: String, path: PathBuf },

    #[error("duplicate id `{id}` for kind `{kind}` (existing: {existing}, new: {incoming})")]
    DuplicateId {
        kind: SpecKind,
        id: String,
        existing: PathBuf,
        incoming: PathBuf,
    },

    #[error(transparent)]
    Spec(#[from] OrchestraProtoError),
}

#[derive(Debug, Error)]
pub enum FrontmatterErrorKind {
    #[error("missing opening `+++` delimiter")]
    MissingOpening,
    #[error("missing closing `+++` delimiter")]
    MissingClosing,
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
}
