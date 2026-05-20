//! L1 Rust syntax parser — tree-sitter based.
//!
//! Public entry point: [`RustParser`] (implements [`AstParser`]).
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | `context` | `WalkCtx` — parse state + metadata helpers |
//! | `registry` | `NodeHandler` trait + `HandlerRegistry` + `dispatch_node` |
//! | `walker` | `walk_item_list`, `walk_body_with` — shared traversal helpers |
//! | `calls` | `collect_calls`, `resolve_callee` — call-graph extraction |
//! | `fields` | `extract_fields`, `push_field` — struct/union field extraction |
//! | `util` | `flatten_use_tree`, `child_of_kind` — pure utilities |
//! | `handlers/` | One file per Rust AST item kind (Strategy pattern) |
//! | `tests` | All 25 unit tests |
//!
//! # Extending
//! To add support for a new Rust AST node kind:
//! 1. Create `handlers/<name>.rs` and `impl NodeHandler for <Name>Handler`.
//! 2. Add one `r.register(...)` line in `handlers/mod.rs::build_registry()`.
//! 3. No other files change.

use tree_sitter::{Language, Parser};

use super::{AstParser, ParseResult};
use crate::CortexError;

pub mod calls;
pub mod context;
pub mod fields;
pub mod handlers;
pub mod registry;
pub mod util;
pub mod walker;

pub use context::WalkCtx;
pub use registry::{HandlerRegistry, NodeHandler};

// ── Public entry point ──────────────────────────────────────────────────────

/// Tree-sitter Rust source file parser.
pub struct RustParser;

impl RustParser {
    pub const EXTENSIONS: &'static [&'static str] = &["rs"];

    fn language() -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }
}

impl AstParser for RustParser {
    fn lang(&self) -> &'static str { "rust" }
    fn extensions(&self) -> &'static [&'static str] { Self::EXTENSIONS }

    fn parse(
        &self,
        file_path: &str,
        source: &[u8],
        preview_bytes: usize,
    ) -> Result<ParseResult, CortexError> {
        let mut parser = Parser::new();
        parser.set_language(&Self::language()).map_err(|e| CortexError::Parse {
            path: file_path.into(),
            detail: e.to_string(),
        })?;
        let tree = parser.parse(source, None).ok_or_else(|| CortexError::Parse {
            path: file_path.into(),
            detail: "tree-sitter returned None".into(),
        })?;

        let registry = handlers::build_registry();
        let mut ctx  = WalkCtx::new(source, file_path, preview_bytes);
        walker::walk_item_list(
            tree.root_node(),
            &mut ctx,
            None,
            &|n, c, p| registry::dispatch_node(&registry, n, c, p),
        );
        Ok(ctx.result)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
