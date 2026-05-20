//! `ImplHandler` — impl_item (inherent + trait impls).

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;
use crate::ast::rust::walker::walk_body_with;

pub struct ImplHandler;

impl NodeHandler for ImplHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        dispatch:  &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let self_type  = ctx.field_text(node, "type").unwrap_or("<impl>").to_string();
        let trait_name = ctx.field_text(node, "trait").map(str::to_string);
        let display    = match &trait_name {
            Some(t) => format!("{t} for {self_type}"),
            None    => self_type.clone(),
        };
        let line   = node.start_position().row as u32 + 1;
        // Line in ID differentiates multiple `impl Foo {}` blocks in the same file.
        let id     = ctx.make_id("Impl", &display, line);
        let cn     = ctx.build(id.clone(), display, CodeNodeKind::Impl, node);
        let own_id = ctx.push(cn, parent_id);

        if let Some(body) = node.child_by_field_name("body") {
            walk_body_with(body, ctx, &own_id, dispatch);
        }
    }
}
