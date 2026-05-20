//! Python AST parser — tree-sitter based.
//!
//! Extracts functions, classes, and import statements from `.py` files.

use tree_sitter::{Language, Node, Parser};
use uuid::Uuid;

use owl_protocol::code::{CodeNode, CodeNodeKind};

use crate::ast::{AstParser, ParseResult};
use crate::CortexError;

const NS: Uuid = Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1,
    0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

/// Tree-sitter Python source file parser.
pub struct PythonParser;

impl PythonParser {
    pub const EXTENSIONS: &'static [&'static str] = &["py"];

    fn language() -> Language {
        tree_sitter_python::LANGUAGE.into()
    }
}

impl AstParser for PythonParser {
    fn lang(&self) -> &'static str { "python" }
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

        let mut result = ParseResult::default();
        walk(tree.root_node(), file_path, source, preview_bytes, None, &mut result);
        Ok(result)
    }
}

// ── Recursive walker ─────────────────────────────────────────────────────────

fn walk(
    node: Node<'_>,
    file_path: &str,
    source: &[u8],
    preview_bytes: usize,
    parent_id: Option<&str>,
    result: &mut ParseResult,
) {
    match node.kind() {
        // `@decorator\ndef foo():` wraps a function_definition or class_definition.
        "decorated_definition" => {
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                match child.kind() {
                    "function_definition" | "class_definition" => {
                        walk(child, file_path, source, preview_bytes, parent_id, result);
                        return;
                    }
                    _ => {}
                }
            }
        }

        "function_definition" => {
            if let Some(name) = field_text(node, "name", source) {
                let id = push_node(
                    node, file_path, source, preview_bytes, name, CodeNodeKind::Function,
                    parent_id, result,
                );
                walk_children(node, file_path, source, preview_bytes, Some(id.as_str()), result);
            }
        }

        "class_definition" => {
            if let Some(name) = field_text(node, "name", source) {
                let id = push_node(
                    node, file_path, source, preview_bytes, name, CodeNodeKind::Struct,
                    parent_id, result,
                );
                walk_children(node, file_path, source, preview_bytes, Some(id.as_str()), result);
            }
        }

        // `import foo` → import_refs keyed on parent_id (best-effort).
        "import_statement" => {
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                if child.kind() == "dotted_name" || child.kind() == "aliased_import" {
                    let path = text(child, source).to_string();
                    if let Some(pid) = parent_id {
                        result.import_refs.push((pid.to_string(), path));
                    }
                }
            }
        }

        // `from foo import bar` → record module path.
        "import_from_statement" => {
            if let Some(module_node) = node.child_by_field_name("module_name") {
                let path = text(module_node, source).to_string();
                if let Some(pid) = parent_id {
                    result.import_refs.push((pid.to_string(), path));
                }
            }
        }

        _ => {
            walk_children(node, file_path, source, preview_bytes, parent_id, result);
        }
    }
}

fn walk_children(
    node: Node<'_>,
    file_path: &str,
    source: &[u8],
    preview_bytes: usize,
    parent_id: Option<&str>,
    result: &mut ParseResult,
) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        walk(child, file_path, source, preview_bytes, parent_id, result);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn text<'a>(node: Node<'a>, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn field_text<'a>(node: Node<'a>, field: &str, source: &'a [u8]) -> Option<String> {
    node.child_by_field_name(field)
        .map(|n| text(n, source).to_string())
}

fn make_id(file_path: &str, kind: &str, name: &str, line: u32) -> String {
    let compound = format!("{}::{}::{}@L{}", file_path, kind, name, line);
    Uuid::new_v5(&NS, compound.as_bytes()).to_string()
}

fn preview_text(node: Node<'_>, source: &[u8], preview_bytes: usize) -> String {
    let raw = node.utf8_text(source).unwrap_or("");
    if raw.len() <= preview_bytes {
        raw.to_string()
    } else {
        let cut = raw[..preview_bytes].rfind('\n').unwrap_or(preview_bytes);
        raw[..cut].to_string()
    }
}

fn push_node(
    node: Node<'_>,
    file_path: &str,
    source: &[u8],
    preview_bytes: usize,
    name: String,
    kind: CodeNodeKind,
    parent_id: Option<&str>,
    result: &mut ParseResult,
) -> String {
    let line = node.start_position().row as u32 + 1;
    let id = make_id(file_path, &kind.to_string(), &name, line);
    let code_node = CodeNode {
        id: id.clone(),
        file_path: file_path.to_string(),
        name,
        kind,
        start_line: line,
        end_line: node.end_position().row as u32 + 1,
        preview: preview_text(node, source, preview_bytes),
        visibility: "public".to_string(),
        qualifiers: vec![],
        description: String::new(),
    };
    if let Some(pid) = parent_id {
        result.contains.push((pid.to_string(), id.clone()));
    }
    result.nodes.push(code_node);
    id
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::AstParser;

    #[test]
    fn parses_function_and_class() {
        let src = b"
def greet(name: str) -> str:
    return f'Hello, {name}'

class Greeter:
    def greet(self, name: str) -> str:
        return f'Hi {name}'
";
        let parser = PythonParser;
        let result = parser.parse("test.py", src, 200).unwrap();
        let names: Vec<&str> = result.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"greet"), "should find top-level function");
        assert!(names.contains(&"Greeter"), "should find class");
    }

    #[test]
    fn parses_decorated_function() {
        let src = b"@app.route('/hello')\ndef hello(): pass\n";
        let parser = PythonParser;
        let result = parser.parse("app.py", src, 200).unwrap();
        let names: Vec<&str> = result.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"hello"), "should find decorated function");
    }
}
