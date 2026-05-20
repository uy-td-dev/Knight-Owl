//! `FunctionHandler` — function_item, function_signature_item.

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::calls::collect_calls;
use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;

pub struct FunctionHandler;

impl NodeHandler for FunctionHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        _dispatch: &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let name  = ctx.field_text(node, "name").unwrap_or("<anonymous>").to_string();
        let line  = node.start_position().row as u32 + 1;
        let kind  = if parent_id.is_some() { CodeNodeKind::Method } else { CodeNodeKind::Function };
        let id    = ctx.make_id(&format!("{kind:?}"), &name, line);
        let cnode = ctx.build(id.clone(), name, kind, node);
        let owner = ctx.push(cnode, parent_id);

        if let Some(body) = node.child_by_field_name("body") {
            collect_calls(ctx, body, &owner);
        }
    }
}
