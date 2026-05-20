//! Docker-backed sandbox using the `bollard` crate (R-21).
//!
//! Flow per `run()` call:
//!   1. Connect to local Docker daemon.
//!   2. Create a container with the workspace bind-mounted **read-only**.
//!   3. Start the container.
//!   4. Wait for exit (with wall-clock timeout).
//!   5. Collect stdout + stderr (last 4 KiB each).
//!   6. Remove the container.
//!   7. Return [`ExecutionOutcome`].
//!
//! Network is disabled by default (`allow_network = false`).

use std::time::{Duration, Instant};

use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, WaitContainerOptions,
};
use bollard::models::HostConfig;
use bollard::Docker;
use futures_util::StreamExt;
use tracing::{debug, warn};

use owl_protocol::sandbox::{ExecutionOutcome, ExecutionPlan, Sandbox, SandboxError};

/// Docker-backed sandbox — production implementation of [`Sandbox`].
pub struct DockerSandbox;

impl DockerSandbox {
    /// Create a new `DockerSandbox` connecting to the local Docker daemon.
    pub fn new() -> Self {
        Self
    }
}

impl Default for DockerSandbox {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Sandbox for DockerSandbox {
    async fn run(&self, plan: ExecutionPlan) -> Result<ExecutionOutcome, SandboxError> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| SandboxError::Docker(e.to_string()))?;

        let timeout = Duration::from_secs(plan.timeout_secs);
        let cmd: Vec<&str> = plan.command.iter().map(String::as_str).collect();
        let env: Vec<String> = plan
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let env_refs: Vec<&str> = env.iter().map(String::as_str).collect();

        let network_mode = if plan.allow_network {
            "bridge".to_string()
        } else {
            "none".to_string()
        };

        let binds = vec![format!("{}:/workspace:ro", plan.workspace_path)];

        let host_cfg = HostConfig {
            binds: Some(binds),
            network_mode: Some(network_mode),
            // Cap memory and CPU to prevent resource exhaustion.
            memory: Some(512 * 1024 * 1024), // 512 MiB
            nano_cpus: Some(1_000_000_000),   // 1 CPU
            ..Default::default()
        };

        let create_opts = CreateContainerOptions::<String> { name: String::new(), platform: None };
        let container_cfg = Config {
            image: Some(plan.image.as_str()),
            cmd: Some(cmd),
            env: Some(env_refs),
            working_dir: Some("/workspace"),
            host_config: Some(host_cfg),
            network_disabled: Some(!plan.allow_network),
            ..Default::default()
        };

        debug!(task_id = %plan.task_id, "creating container");
        let container = docker
            .create_container(Some(create_opts), container_cfg)
            .await
            .map_err(|e| SandboxError::Docker(e.to_string()))?;

        let id = container.id.clone();

        docker
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| SandboxError::Docker(e.to_string()))?;

        let start = Instant::now();

        // Wait for the container to exit, with a hard timeout.
        let exit_code = tokio::time::timeout(
            timeout,
            wait_for_exit(&docker, &id),
        )
        .await
        .map_err(|_| {
            warn!(task_id = %plan.task_id, "sandbox timeout");
            SandboxError::Timeout(plan.timeout_secs)
        })?
        .map_err(|e| SandboxError::Container(e.to_string()))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        // Collect logs.
        let (stdout, stderr) = collect_logs(&docker, &id).await;

        // Always remove the container.
        let _ = docker
            .remove_container(
                &id,
                Some(RemoveContainerOptions { force: true, ..Default::default() }),
            )
            .await;

        debug!(task_id = %plan.task_id, exit_code, duration_ms, "sandbox done");

        Ok(ExecutionOutcome {
            task_id: plan.task_id,
            exit_code,
            stdout,
            stderr,
            duration_ms,
            success: exit_code == 0,
        })
    }
}

async fn wait_for_exit(docker: &Docker, id: &str) -> Result<i64, bollard::errors::Error> {
    let mut stream =
        docker.wait_container(id, None::<WaitContainerOptions<String>>);
    while let Some(result) = stream.next().await {
        let response = result?;
        return Ok(response.status_code);
    }
    Ok(-1)
}

async fn collect_logs(docker: &Docker, id: &str) -> (String, String) {
    let opts = LogsOptions::<String> {
        stdout: true,
        stderr: true,
        tail: "100".into(),
        ..Default::default()
    };
    let mut stdout_chunks: Vec<String> = Vec::new();
    let mut stderr_chunks: Vec<String> = Vec::new();
    let mut stream = docker.logs(id, Some(opts));
    while let Some(Ok(chunk)) = stream.next().await {
        use bollard::container::LogOutput;
        match chunk {
            LogOutput::StdOut { message } => {
                stdout_chunks.push(String::from_utf8_lossy(&message).into_owned());
            }
            LogOutput::StdErr { message } => {
                stderr_chunks.push(String::from_utf8_lossy(&message).into_owned());
            }
            _ => {}
        }
    }
    let stdout = truncate_tail(stdout_chunks.join(""), 4096);
    let stderr = truncate_tail(stderr_chunks.join(""), 4096);
    (stdout, stderr)
}

/// Keep only the last `max_bytes` bytes (UTF-8 safe truncation).
fn truncate_tail(s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    let start = s.len() - max_bytes;
    // Walk forward to a valid UTF-8 boundary.
    let boundary = s[start..].char_indices().next().map(|(i, _)| start + i).unwrap_or(start);
    s[boundary..].to_string()
}
