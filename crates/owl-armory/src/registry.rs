//! Tool registry — the single file that knows all native tools.
//!
//! Adding a tool = one `push` here, nothing else.

use std::path::PathBuf;
use std::sync::Arc;

use owl_protocol::sandbox::Sandbox;

use crate::tools::apply_patch::ApplyPatchTool;
use crate::tools::bash::BashTool;
use crate::tools::echo::EchoTool;
use crate::tools::edit_file::EditFileTool;
use crate::tools::glob_files::GlobTool;
use crate::tools::grep::GrepTool;
use crate::tools::list_dir::ListDirTool;
use crate::tools::multi_edit::MultiEditTool;
use crate::tools::read_file::ReadFileTool;
use crate::tools::run_command::RunCommandTool;
use crate::tools::write_file::WriteFileTool;
use crate::traits::NativeTool;

/// Build the base registry (EchoTool only).
///
/// Used in tests and contexts where no workspace root is available.
pub fn build_registry() -> Vec<Box<dyn NativeTool>> {
    vec![Box::new(EchoTool)]
}

/// Build the full registry, all tools scoped to `root`.
///
/// Use this in apps after the workspace root is known.  Equivalent to
/// [`build_registry_with_root_and_sandbox`] called with `sandbox = None`
/// — `bash` runs directly on the host with no isolation.
pub fn build_registry_with_root(root: PathBuf) -> Vec<Box<dyn NativeTool>> {
    build_registry_with_root_and_sandbox(root, None)
}

/// Build the full registry; if `sandbox` is `Some`, the `bash` tool routes
/// every command through it for isolation (R-21).  All other tools remain
/// host-direct (file edits are gated by the approval flow + `allowed_root`).
pub fn build_registry_with_root_and_sandbox(
    root:    PathBuf,
    sandbox: Option<Arc<dyn Sandbox>>,
) -> Vec<Box<dyn NativeTool>> {
    let bash: Box<dyn NativeTool> = match sandbox {
        Some(s) => Box::new(BashTool::with_sandbox(root.clone(), s)),
        None    => Box::new(BashTool::new(root.clone())),
    };
    vec![
        Box::new(EchoTool),
        Box::new(ReadFileTool   { allowed_root:   root.clone() }),
        Box::new(WriteFileTool  { allowed_root:   root.clone() }),
        Box::new(EditFileTool   { allowed_root:   root.clone() }),
        Box::new(MultiEditTool  { allowed_root:   root.clone() }),
        Box::new(ApplyPatchTool { allowed_root:   root.clone() }),
        Box::new(ListDirTool    { workspace_root: root.clone() }),
        Box::new(GlobTool       { workspace_root: root.clone() }),
        Box::new(GrepTool       { workspace_root: root.clone() }),
        Box::new(RunCommandTool { workspace_root: root }),
        bash,
    ]
}
