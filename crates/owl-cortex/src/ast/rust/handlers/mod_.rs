//! `ModHandler` — mod_item (inline and declaration-only).

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;
use crate::ast::rust::walker::walk_item_list;

pub struct ModHandler;

impl NodeHandler for ModHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        dispatch:  &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let name   = ctx.field_text(node, "name").unwrap_or("<mod>").to_string();
        let line   = node.start_position().row as u32 + 1;
        let id     = ctx.make_id("Module", &name, line);
        let cn     = ctx.build(id.clone(), name, CodeNodeKind::Module, node);
        let own_id = ctx.push(cn, parent_id);

        if let Some(body) = node.child_by_field_name("body") {
            walk_item_list(body, ctx, Some(&own_id), dispatch);
        }
    }
}
