//! Struct / union field extraction — extract_fields + push_field.

use tree_sitter::Node;

use owl_protocol::code::{CodeNode, CodeNodeKind};

use super::context::WalkCtx;

/// Extract `field_declaration` children from a struct/union body.
pub fn extract_fields<'src>(ctx: &mut WalkCtx<'src>, struct_node: Node<'src>, struct_id: &str) {
    let body = match struct_node.child_by_field_name("body") {
        Some(b) => b,
        None    => return,
    };

    if body.kind() == "field_declaration_list" {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() == "field_declaration" {
                push_field(ctx, child, struct_id);
            }
        }
    }

    // Tuple struct: pair each visibility_modifier with its following type.
    if body.kind() == "ordered_field_declaration_list" {
        let mut idx = 0u32;
        let mut pending_vis = String::new();
        for i in 0..body.child_count() {
            let child = match body.child(i) { Some(c) => c, None => continue };
            match child.kind() {
                "visibility_modifier" => {
                    pending_vis = ctx.text(child).to_string();
                }
                "(" | ")" | "," | "attribute_item" | "inner_attribute_item" => {}
                _ if child.is_named() => {
                    let type_str = ctx.text(child).to_string();
                    let name = format!("_{idx}");
                    let line = child.start_position().row as u32 + 1;
                    let id   = ctx.make_id("TupleField", &format!("{struct_id}::{name}"), line);
                    let cn = CodeNode {
                        id:          id.clone(),
                        file_path:   ctx.file_path.to_string(),
                        name:        name.clone(),
                        kind:        CodeNodeKind::Variable,
                        start_line:  line,
                        end_line:    child.end_position().row as u32 + 1,
                        preview:     type_str.clone(),
                        visibility:  std::mem::take(&mut pending_vis),
                        qualifiers:  String::new(),
                        description: format!("{name}: {type_str}"),
                    };
                    ctx.result.contains.push((struct_id.to_string(), id));
                    ctx.result.nodes.push(cn);
                    idx += 1;
                }
                _ => {}
            }
        }
    }
}

/// Build and push a single `field_declaration` node.
pub fn push_field<'src>(ctx: &mut WalkCtx<'src>, node: Node<'src>, struct_id: &str) {
    let fname    = ctx.field_text(node, "name").unwrap_or("_").to_string();
    let type_str = ctx.field_text(node, "type").unwrap_or("").to_string();
    let line     = node.start_position().row as u32 + 1;
    let id       = ctx.make_id("Field", &format!("{struct_id}::{fname}"), line);
    let cn = CodeNode {
        id:          id.clone(),
        file_path:   ctx.file_path.to_string(),
        name:        fname.clone(),
        kind:        CodeNodeKind::Variable,
        start_line:  line,
        end_line:    node.end_position().row as u32 + 1,
        preview:     format!("{fname}: {type_str}"),
        visibility:  ctx.visibility_of(node),
        qualifiers:  String::new(),
        description: ctx.doc_comment_above(node),
    };
    ctx.result.contains.push((struct_id.to_string(), id));
    ctx.result.nodes.push(cn);
}
