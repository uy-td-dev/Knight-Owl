//! L1 Syntax + L2 Logic types for the cortex knowledge-graph layer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A source file tracked by the cortex.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileNode {
    /// Canonical file path relative to the workspace root.
    pub path: String,
    /// Detected language (e.g. `"rust"`, `"typescript"`, `"python"`).
    pub lang: String,
    /// SHA-256 hex of the file content — used for change detection.
    pub content_hash: String,
}

/// Category of a parsed code element (L1 AST node kind).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodeNodeKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Constant,
    Variable,
    TypeAlias,
    Import,
    Other(String),
}

impl std::fmt::Display for CodeNodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodeNodeKind::Other(s) => write!(f, "{s}"),
            _ => write!(f, "{self:?}"),
        }
    }
}

/// A code element node in the L1 syntax graph.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CodeNode {
    /// Stable deterministic id: UUIDv5 of `"{path}::{kind}::{name}@L{line}"`.
    pub id: String,
    /// File path this node lives in.
    pub file_path: String,
    /// Unqualified name (e.g. `"my_function"`, `"MyStruct"`).
    pub name: String,
    /// Node category.
    pub kind: CodeNodeKind,
    /// 1-indexed start line in the source file.
    pub start_line: u32,
    /// 1-indexed end line in the source file.
    pub end_line: u32,
    /// First ~200 chars of the node's source text (for context).
    pub preview: String,
    /// Visibility modifier: `""` (private) | `"pub"` | `"pub(crate)"` | `"pub(super)"`.
    pub visibility: String,
    /// Function qualifiers: `""` | `"async"` | `"unsafe"` | `"async unsafe"` | `"const"` etc.
    pub qualifiers: String,
    /// Concatenated doc comments (`///`) immediately above this item.
    pub description: String,
}

/// A directed structural edge between two nodes in the code graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CodeEdgeKind {
    /// file → code_node: file declares this top-level item.
    Defines,
    /// code_node → code_node: one item contains another (e.g. impl → method).
    Contains,
    /// code_node → code_node: one item invokes another (syntax-level, unresolved).
    Calls,
    /// code_node → code_node: use/import reference.
    Imports,
    // L2 edges — populated by LSP
    /// code_node → code_node: cross-file resolved reference.
    References,
    /// code_node → code_node: struct/enum implements a trait.
    Implements,
    /// code_node → code_node: method overrides a trait default.
    Overrides,
}

/// An edge in the code graph.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CodeEdge {
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    /// Edge kind.
    pub kind: CodeEdgeKind,
}
