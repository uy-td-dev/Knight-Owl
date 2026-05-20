//! `ExternCrateHandler` — extern_crate_declaration.

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;

pub struct ExternCrateHandler;

impl NodeHandler for ExternCrateHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        _dispatch: &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let name = ctx.field_text(node, "name").unwrap_or("<crate>").to_string();
        let line = node.start_position().row as u32 + 1;
        let id   = ctx.make_id("ExternCrate", &name, line);
        let cn   = ctx.build(id, name, CodeNodeKind::Import, node);
        ctx.push(cn, parent_id);
    }
}
