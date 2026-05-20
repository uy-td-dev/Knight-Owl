//! Traversal helpers shared by recursive handlers.
//!
//! `walk_item_list` and `walk_body_with` are free functions so any handler
//! can call them via the `dispatch` closure without importing the registry.

use tree_sitter::Node;

use super::context::WalkCtx;

/// Walk all named children of a container node and dispatch each.
pub fn walk_item_list<'src>(
    container: Node<'src>,
    ctx:       &mut WalkCtx<'src>,
    parent_id: Option<&str>,
    dispatch:  &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
) {
    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        dispatch(child, ctx, parent_id);
    }
}

/// Walk the `declaration_list` body of an impl or trait block.
///
/// Only dispatches item kinds legal inside impl/trait bodies; attribute items
/// and other non-item children are skipped.
pub fn walk_body_with<'src>(
    body:      Node<'src>,
    ctx:       &mut WalkCtx<'src>,
    parent_id: &str,
    dispatch:  &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "function_item"
            | "function_signature_item"
            | "const_item"
            | "static_item"
            | "type_item"
            | "associated_type" => {
                dispatch(child, ctx, Some(parent_id));
            }
            "attribute_item" | "inner_attribute_item" => {}
            _ => {}
        }
    }
}
