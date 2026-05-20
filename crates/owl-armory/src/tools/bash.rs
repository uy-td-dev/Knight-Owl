//! `BashTool` — run an arbitrary shell command in the workspace.
//!
//! Wider scope than `RunCommandTool` (which whitelists cargo/git only).
//! A small denylist blocks the obvious foot-guns (`rm -rf /`, `:(){:|:&};:`,
//! and outbound network abuse) but otherwise the tool is permissive — the
//! same way Claude Code's `Bash` tool is.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::time::timeout;

use owl_protocol::sandbox::{ExecutionPlan, Sandbox};
use owl_protocol::tools::{ToolCall, ToolResult};

use crate::traits::NativeTool;
use crate::ArmoryError;

const DEFAULT_TIMEOUT_MS: u64 = 60_000;
const MAX_TIMEOUT_MS:     u64 = 600_000;

/// Patterns that are immediately rejected.
const DENYLIST: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    ":(){:|:&};:",   // fork bomb
    "mkfs",
    "dd if=",
    "> /dev/sda",
    "> /dev/nvme",
    "shutdown",
    "reboot",
];

pub struct BashTool {
    pub workspace_root: PathBuf,
    /// Optional sandbox.  When set, every `bash` call is routed through
    /// [`Sandbox::run`] instead of `tokio::process` directly.  Wire a
    /// `LocalSandbox` for plain isolation, or `DockerSandbox` for full
    /// container isolation (R-21).
    pub sandbox: Option<Arc<dyn Sandbox>>,
}

impl BashTool {
    /// Construct without a sandbox (legacy direct-host execution).
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root, sandbox: None }
    }

    /// Construct with a sandbox; all calls are routed through it.
    pub fn with_sandbox(workspace_root: PathBuf, sandbox: Arc<dyn Sandbox>) -> Self {
        Self { workspace_root, sandbox: Some(sandbox) }
    }
}

#[derive(Deserialize)]
struct Args {
    /// Shell command line.  Accepts `cmd` / `shell` / `script` / `code`
    /// aliases since LLMs often hallucinate the field name.
    #[serde(alias = "cmd", alias = "shell", alias = "script", alias = "code")]
    command:    String,
    #[serde(default, alias = "timeout")]
    timeout_ms: Option<u64>,
}

#[async_trait]
impl NativeTool for BashTool {
    fn name(&self) -> &'static str { "bash" }

    fn description(&self) -> &'static str {
        "Run a shell command (via /bin/sh -c) in the workspace root. \
         Returns stdout, stderr, and exit code.  Default timeout 60s; \
         override with `timeout_ms`."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        for pat in DENYLIST {
            if args.command.contains(pat) {
                return Err(ArmoryError::InvalidArgs(format!(
                    "command rejected by denylist (`{pat}`)"
                )));
            }
        }

        let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS);

        // Sandbox path (R-21): defer to the injected backend.  The plan is
        // declarative — image / network / mounts are owned by the sandbox
        // backend's defaults; we only supply the command + workspace.
        if let Some(sandbox) = &self.sandbox {
            let plan = ExecutionPlan {
                image: "rust:1.78-slim".into(), // no-op for LocalSandbox
                command: vec!["/bin/sh".into(), "-c".into(), args.command.clone()],
                workspace_path: self.workspace_root.display().to_string(),
                timeout_secs: (timeout_ms / 1000).max(1),
                allow_network: false,
                env: std::collections::HashMap::new(),
                task_id: uuid::Uuid::new_v4().to_string(),
            };
            let outcome = sandbox.run(plan).await
                .map_err(|e| ArmoryError::Execution(e.to_string()))?;
            return Ok(ToolResult::ok(self.name(), serde_json::json!({
                "stdout":    outcome.stdout,
                "stderr":    outcome.stderr,
                "exit_code": outcome.exit_code,
                "success":   outcome.success,
                "sandboxed": true,
            })));
        }

        // Legacy host path — used when no sandbox is wired.
        let mut child = tokio::process::Command::new("/bin/sh");
        child
            .arg("-c")
            .arg(&args.command)
            .current_dir(&self.workspace_root);

        let exec = child.output();
        let result = timeout(Duration::from_millis(timeout_ms), exec).await;

        let output = match result {
            Ok(Ok(out)) => out,
            Ok(Err(e))  => return Err(ArmoryError::Execution(e.to_string())),
            Err(_)      => return Err(ArmoryError::Execution(format!(
                "command timed out after {timeout_ms} ms"
            ))),
        };

        let stdout = truncate(&String::from_utf8_lossy(&output.stdout), 32_000);
        let stderr = truncate(&String::from_utf8_lossy(&output.stderr), 8_000);

        Ok(ToolResult::ok(self.name(), serde_json::json!({
            "stdout":    stdout,
            "stderr":    stderr,
            "exit_code": output.status.code().unwrap_or(-1),
            "success":   output.status.success(),
            "sandboxed": false,
        })))
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() }
    else { format!("{}\n…[truncated {} bytes]", &s[..max], s.len() - max) }
}
