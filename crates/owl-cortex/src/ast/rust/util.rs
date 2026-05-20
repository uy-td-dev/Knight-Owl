//! Pure utility functions — use-tree flattening and tree navigation.

use tree_sitter::Node;

/// Recursively flatten a `use_tree` node into individual import path strings.
pub fn flatten_use_tree(node: Node<'_>, source: &[u8], prefix: &str, out: &mut Vec<String>) {
    fn text<'a>(n: Node<'_>, src: &'a [u8]) -> &'a str {
        n.utf8_text(src).unwrap_or("_")
    }
    fn join(prefix: &str, name: &str) -> String {
        if prefix.is_empty() { name.to_string() } else { format!("{prefix}::{name}") }
    }

    match node.kind() {
        "identifier" => {
            out.push(join(prefix, text(node, source)));
        }
        "scoped_identifier" => {
            out.push(join(prefix, text(node, source)));
        }
        "scoped_use_list" => {
            let path = node
                .child_by_field_name("path")
                .map(|n| text(n, source))
                .unwrap_or("");
            let new_prefix = join(prefix, path);
            if let Some(list) = node.child_by_field_name("list") {
                flatten_use_tree(list, source, &new_prefix, out);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                flatten_use_tree(child, source, prefix, out);
            }
        }
        "use_wildcard" => {
            out.push(join(prefix, "*"));
        }
        "use_as_clause" => {
            let path  = node.child_by_field_name("path").map(|n| text(n, source)).unwrap_or("_");
            let alias = node.child_by_field_name("alias").map(|n| text(n, source)).unwrap_or("_");
            out.push(format!("{} as {alias}", join(prefix, path)));
        }
        "self" => {
            out.push(if prefix.is_empty() { "self".into() } else { prefix.to_string() });
        }
        _ => {
            out.push(join(prefix, text(node, source)));
        }
    }
}

/// Return the first direct child (including anonymous) with the given kind.
pub fn child_of_kind<'src>(node: Node<'src>, kind: &str) -> Option<Node<'src>> {
    for i in 0..node.child_count() {
        if let Some(c) = node.child(i) {
            if c.kind() == kind {
                return Some(c);
            }
        }
    }
    None
}
