//! Slash-command specification — user-invokable shortcut that expands into
//! a prompt rewrite, a workflow run, or a direct tool call.

use serde::{Deserialize, Serialize};

use super::ids::{CommandId, ToolName, WorkflowId};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CommandSpec {
    /// Spec format version.
    pub schema_version: u32,

    /// Stable id; also the slash-command name shown in the `/`-menu (so
    /// `id = "audit"` is invoked as `/audit`).
    pub id: CommandId,
    /// Display name (currently equals `id` but kept separate for future i18n).
    pub name: String,
    pub description: String,

    /// What the command expands to when invoked.
    pub expansion: CommandExpansion,
}

/// Three kinds of expansion — keep mutually exclusive: a single command
/// either rewrites the user's prompt, triggers a workflow, or fires a tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandExpansion {
    /// Rewrite the user's draft using a template.  Supports positional args
    /// `{{$1}}, {{$2}}, …` parsed from the line after the command name.
    Text { template: String },

    /// Run the named workflow with the rest of the line as `{{user_input}}`.
    Workflow { workflow: WorkflowId },

    /// Fire a single tool with pre-baked args (advanced — use sparingly).
    /// Args are JSON merged with any user-provided override.
    Tool { name: ToolName, args: serde_json::Value },
}
