//! `MacroDefHandler` — macro_definition (macro_rules!).

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;

pub struct MacroDefHandler;

impl NodeHandler for MacroDefHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        _dispatch: &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let name = ctx.field_text(node, "name").unwrap_or("<macro>").to_string();
        let line = node.start_position().row as u32 + 1;
        let id   = ctx.make_id("Macro", &name, line);
        let cn   = ctx.build(id, name, CodeNodeKind::Other("macro_rules".into()), node);
        ctx.push(cn, parent_id);
    }
}
