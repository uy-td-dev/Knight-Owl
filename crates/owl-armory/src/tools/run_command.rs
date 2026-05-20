//! `RunCommandTool` — run whitelisted shell commands (cargo, git) in the workspace.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use owl_protocol::tools::{ToolCall, ToolResult};

use crate::traits::NativeTool;
use crate::ArmoryError;

/// Allowed top-level programs and their permitted subcommands.
const ALLOWED: &[(&str, &[&str])] = &[
    ("cargo", &["check", "build", "test", "fmt", "clippy", "doc", "run"]),
    ("git",   &["status", "diff", "log", "show", "blame"]),
];

/// Runs a whitelisted command in the workspace root and returns stdout/stderr.
pub struct RunCommandTool {
    /// Working directory for command execution.
    pub workspace_root: PathBuf,
}

#[derive(Deserialize)]
struct Args {
    program: String,
    #[serde(default)]
    args: Vec<String>,
}

#[async_trait]
impl NativeTool for RunCommandTool {
    fn name(&self) -> &'static str { "run_command" }

    fn description(&self) -> &'static str {
        "Run a whitelisted command (cargo or git) in the workspace root. \
         Returns stdout, stderr, and exit code."
    }

    async fn run(&self, call: ToolCall) -> Result<ToolResult, ArmoryError> {
        let args: Args = Self::parse_args(call.args)?;

        validate(&args.program, &args.args)?;

        let output = tokio::process::Command::new(&args.program)
            .args(&args.args)
            .current_dir(&self.workspace_root)
            .output()
            .await
            .map_err(|e| ArmoryError::Execution(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let code   = output.status.code().unwrap_or(-1);

        Ok(ToolResult::ok(self.name(), serde_json::json!({
            "stdout":    stdout,
            "stderr":    stderr,
            "exit_code": code,
            "success":   output.status.success(),
        })))
    }
}

fn validate(program: &str, args: &[String]) -> Result<(), ArmoryError> {
    let allowed_cmds = ALLOWED
        .iter()
        .find(|(p, _)| *p == program)
        .map(|(_, cmds)| *cmds)
        .ok_or_else(|| ArmoryError::InvalidArgs(
            format!("program `{program}` not allowed; use: cargo, git")
        ))?;

    let sub = args.first().map(String::as_str).unwrap_or("");
    if !sub.is_empty() && !allowed_cmds.contains(&sub) {
        return Err(ArmoryError::InvalidArgs(
            format!("`{program} {sub}` not allowed")
        ));
    }
    Ok(())
}
