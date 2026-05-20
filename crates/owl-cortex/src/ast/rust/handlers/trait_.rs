//! `TraitHandler` — trait_item.

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;
use crate::ast::rust::walker::walk_body_with;

pub struct TraitHandler;

impl NodeHandler for TraitHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        dispatch:  &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let name   = ctx.field_text(node, "name").unwrap_or("<trait>").to_string();
        let line   = node.start_position().row as u32 + 1;
        let id     = ctx.make_id("Trait", &name, line);
        let cn     = ctx.build(id.clone(), name, CodeNodeKind::Trait, node);
        let own_id = ctx.push(cn, parent_id);

        if let Some(body) = node.child_by_field_name("body") {
            walk_body_with(body, ctx, &own_id, dispatch);
        }
    }
}
