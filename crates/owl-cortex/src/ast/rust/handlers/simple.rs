//! `SimpleHandler` — const_item, static_item, type_item / associated_type.

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;

/// Handles named items with no interesting body: constants, statics, type aliases.
///
/// Parameterised by `CodeNodeKind` so one struct covers three tree-sitter kinds.
pub struct SimpleHandler(pub CodeNodeKind);

impl NodeHandler for SimpleHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        _dispatch: &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let name = ctx.field_text(node, "name").unwrap_or("<anonymous>").to_string();
        let line = node.start_position().row as u32 + 1;
        let id   = ctx.make_id(&format!("{:?}", self.0), &name, line);
        let cn   = ctx.build(id, name, self.0.clone(), node);
        ctx.push(cn, parent_id);
    }
}
