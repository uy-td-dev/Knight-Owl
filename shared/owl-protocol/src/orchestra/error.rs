//! Errors emitted while parsing / validating orchestra spec files.
//!
//! Loader / registry failures (I/O, file not found, parse errors) live in the
//! `owl-orchestra` crate.  Anything that can be detected from the data alone
//! lives here so spec validation is reusable across crates.

use thiserror::Error;

/// Validation errors that can be raised while constructing strongly-typed
/// orchestra entities from raw user input.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OrchestraProtoError {
    /// Identifier failed the `^[a-z][a-z0-9_-]{1,63}$` pattern.
    #[error("invalid id `{0}` — must match ^[a-z][a-z0-9_-]{{1,63}}$")]
    InvalidId(String),

    /// Identifier matches one of the reserved built-in names.
    #[error("reserved id `{0}` may not be used by user-defined entities")]
    ReservedId(String),

    /// File schema_version exceeds what this build supports.
    #[error("schema_version {found} > supported {supported}; please upgrade Knight-Owl")]
    SchemaVersionTooNew { found: u32, supported: u32 },

    /// File schema_version is missing — every spec file must declare it.
    #[error("missing required field `schema_version`")]
    MissingSchemaVersion,

    /// A field cross-referencing another entity points to an id that doesn't
    /// exist in the registry yet.  Raised during the resolve phase.
    #[error("unresolved reference: {kind} `{id}`")]
    UnresolvedReference { kind: &'static str, id: String },

    /// Workflow `depends` list contains a step id that wasn't declared.
    #[error("workflow step `{step}` depends on unknown step `{missing}`")]
    UnknownStepDependency { step: String, missing: String },

    /// Workflow has a cycle in its DAG.
    #[error("workflow contains a dependency cycle involving `{step}`")]
    CyclicWorkflow { step: String },

    /// Empty / nonsensical spec — e.g. agent with no system prompt.
    #[error("spec validation failed: {0}")]
    Validation(String),
}
