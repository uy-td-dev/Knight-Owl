//! `StructHandler` — struct_item, union_item.

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::fields::extract_fields;
use crate::ast::rust::registry::NodeHandler;

pub struct StructHandler;

impl NodeHandler for StructHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        _dispatch: &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let name   = ctx.field_text(node, "name").unwrap_or("<anonymous>").to_string();
        let line   = node.start_position().row as u32 + 1;
        let id     = ctx.make_id("Struct", &name, line);
        let cn     = ctx.build(id.clone(), name, CodeNodeKind::Struct, node);
        let own_id = ctx.push(cn, parent_id);
        extract_fields(ctx, node, &own_id);
    }
}
