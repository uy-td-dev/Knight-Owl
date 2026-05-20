//! `WalkCtx` — shared parse context threaded through every handler.
//!
//! Owns source bytes, file metadata, and the result accumulator.
//! Exposes all metadata-extraction helpers used by visitor logic.

use tree_sitter::Node;
use uuid::Uuid;

use owl_protocol::code::{CodeNode, CodeNodeKind};

use crate::ast::ParseResult;

use super::util::child_of_kind;

// ── UUID namespace (arbitrary, fixed) ─────────────────────────────────────
const NS: Uuid = Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1,
    0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

/// Shared parse context threaded through every handler.
pub struct WalkCtx<'src> {
    pub source:        &'src [u8],
    pub file_path:     &'src str,
    pub preview_bytes: usize,
    pub result:        ParseResult,
}

impl<'src> WalkCtx<'src> {
    pub fn new(source: &'src [u8], file_path: &'src str, preview_bytes: usize) -> Self {
        Self { source, file_path, preview_bytes, result: ParseResult::default() }
    }

    // ── Node construction ──────────────────────────────────────────────────

    /// Build a `CodeNode` from an AST node (extracts all metadata).
    pub fn build(
        &self,
        id:   String,
        name: String,
        kind: CodeNodeKind,
        node: Node<'src>,
    ) -> CodeNode {
        CodeNode {
            id,
            file_path:   self.file_path.to_string(),
            name,
            kind,
            start_line:  node.start_position().row as u32 + 1,
            end_line:    node.end_position().row as u32 + 1,
            preview:     self.preview(node),
            visibility:  self.visibility_of(node),
            qualifiers:  self.qualifiers_of(node),
            description: self.doc_comment_above(node),
        }
    }

    /// Push a `CodeNode`, emit a CONTAINS edge to `parent_id`, return the id.
    pub fn push(&mut self, node: CodeNode, parent_id: Option<&str>) -> String {
        let id = node.id.clone();
        if let Some(pid) = parent_id {
            self.result.contains.push((pid.to_string(), id.clone()));
        }
        self.result.nodes.push(node);
        id
    }

    // ── ID generation ──────────────────────────────────────────────────────

    /// Deterministic UUIDv5: `"{file}::{kind}::{name}@L{line}"`.
    pub fn make_id(&self, kind: &str, name: &str, line: u32) -> String {
        let compound = format!("{}::{}::{}@L{}", self.file_path, kind, name, line);
        Uuid::new_v5(&NS, compound.as_bytes()).to_string()
    }

    // ── Text extraction ────────────────────────────────────────────────────

    /// Raw UTF-8 text of a node (empty string on invalid UTF-8).
    pub fn text(&self, node: Node<'src>) -> &'src str {
        node.utf8_text(self.source).unwrap_or("")
    }

    /// UTF-8 text of a named field, or `None` if absent.
    pub fn field_text(&self, node: Node<'src>, field: &str) -> Option<&'src str> {
        node.child_by_field_name(field)
            .and_then(|n| n.utf8_text(self.source).ok())
    }

    /// First `preview_bytes` of the node's first non-empty line.
    pub fn preview(&self, node: Node<'src>) -> String {
        let raw   = self.text(node);
        let first = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        if first.len() <= self.preview_bytes {
            first.to_string()
        } else {
            format!("{}…", &first[..self.preview_bytes])
        }
    }

    // ── Metadata extractors ────────────────────────────────────────────────

    /// Extract `pub` / `pub(crate)` / `pub(super)` from a `visibility_modifier` child.
    pub fn visibility_of(&self, node: Node<'src>) -> String {
        child_of_kind(node, "visibility_modifier")
            .map(|n| self.text(n).to_string())
            .unwrap_or_default()
    }

    /// Extract `async` / `unsafe` / `const` / `extern "ABI"` qualifiers.
    pub fn qualifiers_of(&self, node: Node<'src>) -> String {
        child_of_kind(node, "function_modifiers").map(|mods| {
            let count = mods.child_count();
            let mut parts = Vec::new();
            for i in 0..count {
                if let Some(c) = mods.child(i) {
                    if matches!(c.kind(), "async" | "unsafe" | "const" | "extern_modifier") {
                        parts.push(self.text(c).to_string());
                    }
                }
            }
            parts.join(" ")
        }).unwrap_or_default()
    }

    /// Collect consecutive `///` doc comments immediately above `node`.
    ///
    /// Traverses `prev_named_sibling()` backwards, skipping `attribute_item` nodes,
    /// collecting `line_comment` nodes that begin with `///`.
    pub fn doc_comment_above(&self, node: Node<'src>) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut cur = node.prev_named_sibling();
        while let Some(sib) = cur {
            match sib.kind() {
                "line_comment" => {
                    let text = self.text(sib);
                    if let Some(rest) = text.strip_prefix("///") {
                        lines.push(rest.trim().to_string());
                        cur = sib.prev_named_sibling();
                    } else {
                        break;
                    }
                }
                "attribute_item" | "inner_attribute_item" => {
                    cur = sib.prev_named_sibling();
                }
                _ => break,
            }
        }
        lines.reverse();
        lines.join("\n")
    }
}

