//! Local (no-Docker) sandbox — runs commands directly via `tokio::process`.
//!
//! Use this when Docker is unavailable (CI without Docker, development machine
//! where the daemon isn't running).  Does **not** provide isolation — the
//! command runs with the current user's permissions on the host filesystem.
//!
//! Wire-up: `LocalSandbox` is the default; `DockerSandbox` is preferred when
//! Docker is reachable.  Selection happens at startup in the app layer.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::process::Command;

use owl_protocol::sandbox::{ExecutionOutcome, ExecutionPlan, Sandbox, SandboxError};

/// Local subprocess sandbox — no Docker required.
pub struct LocalSandbox;

impl LocalSandbox {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LocalSandbox {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Sandbox for LocalSandbox {
    async fn run(&self, plan: ExecutionPlan) -> Result<ExecutionOutcome, SandboxError> {
        let [prog, args @ ..] = plan.command.as_slice() else {
            return Err(SandboxError::Io("empty command vector".into()));
        };

        let start = Instant::now();
        let timeout = Duration::from_secs(plan.timeout_secs);

        let output = tokio::time::timeout(
            timeout,
            Command::new(prog)
                .args(args)
                .current_dir(&plan.workspace_path)
                .envs(&plan.env)
                .output(),
        )
        .await
        .map_err(|_| SandboxError::Timeout(plan.timeout_secs))?
        .map_err(|e| SandboxError::Io(e.to_string()))?;

        let duration_ms = start.elapsed().as_millis() as u64;
        let exit_code = output.status.code().unwrap_or(-1) as i64;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        Ok(ExecutionOutcome {
            task_id: plan.task_id,
            exit_code,
            stdout: truncate_tail(stdout, 4096),
            stderr: truncate_tail(stderr, 4096),
            duration_ms,
            success: exit_code == 0,
        })
    }
}

fn truncate_tail(s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    let start = s.len() - max_bytes;
    let boundary = s[start..].char_indices().next().map(|(i, _)| start + i).unwrap_or(start);
    s[boundary..].to_string()
}
