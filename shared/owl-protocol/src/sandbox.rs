//! Sandbox execution types — used by owl-sandbox (impl) and owl-brain (caller).
//!
//! The `Sandbox` trait lives here (R-5) so owl-brain can hold `Arc<dyn Sandbox>`
//! without importing the concrete bollard crate.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Declarative description of work to run inside the sandbox.
///
/// All fields are plain data — no handles or sockets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// Docker image (e.g. `"rust:1.78"`).
    pub image: String,
    /// Argv for the container entry-point — no shell expansion.
    pub command: Vec<String>,
    /// Absolute host path of the workspace to mount.
    /// Mounted **read-only** inside the container at `/workspace`.
    pub workspace_path: String,
    /// Wall-clock timeout in seconds before the container is killed.
    pub timeout_secs: u64,
    /// Set `true` to grant the container network access (default: `false`).
    #[serde(default)]
    pub allow_network: bool,
    /// Extra environment variables injected into the container.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Opaque task id linking the plan to the originating user request.
    pub task_id: String,
}

impl ExecutionPlan {
    /// Convenience constructor for a `cargo check` verification run.
    ///
    /// Uses `rust:1.78-slim` and a 120 s timeout.
    pub fn cargo_check(workspace_path: impl Into<String>, task_id: impl Into<String>) -> Self {
        Self {
            image: "rust:1.78-slim".into(),
            command: vec!["cargo".into(), "check".into(), "--message-format=short".into()],
            workspace_path: workspace_path.into(),
            timeout_secs: 120,
            allow_network: false,
            env: HashMap::new(),
            task_id: task_id.into(),
        }
    }

    /// Convenience constructor for a `cargo test` run.
    pub fn cargo_test(workspace_path: impl Into<String>, task_id: impl Into<String>) -> Self {
        Self {
            image: "rust:1.78-slim".into(),
            command: vec!["cargo".into(), "test".into(), "--".into(), "--nocapture".into()],
            workspace_path: workspace_path.into(),
            timeout_secs: 300,
            allow_network: false,
            env: HashMap::new(),
            task_id: task_id.into(),
        }
    }
}

/// Result of a sandbox execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutcome {
    /// Mirrors `ExecutionPlan::task_id`.
    pub task_id: String,
    /// Container exit code (`0` = success).
    pub exit_code: i64,
    /// Last 4 KiB of stdout.
    pub stdout: String,
    /// Last 4 KiB of stderr.
    pub stderr: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// `true` iff `exit_code == 0`.
    pub success: bool,
}

/// Errors from the sandbox layer.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum SandboxError {
    #[error("docker daemon error: {0}")]
    Docker(String),
    #[error("execution timed out after {0}s")]
    Timeout(u64),
    #[error("image not found: {0}")]
    ImageNotFound(String),
    #[error("container error: {0}")]
    Container(String),
    #[error("io error: {0}")]
    Io(String),
}

/// Pluggable sandbox backend — injected into owl-brain via `Arc<dyn Sandbox>`.
///
/// Concrete implementation: `owl_sandbox::docker::DockerSandbox`.
/// Fallback for environments without Docker: `owl_sandbox::local::LocalSandbox`.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Execute `plan` in isolation and return the outcome.
    async fn run(&self, plan: ExecutionPlan) -> Result<ExecutionOutcome, SandboxError>;
}
