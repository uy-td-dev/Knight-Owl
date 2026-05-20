//! `ForeignModHandler` — foreign_mod_item (`extern "C" { … }`).

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;
use crate::ast::rust::util::child_of_kind;
use crate::ast::rust::walker::walk_item_list;

pub struct ForeignModHandler;

impl NodeHandler for ForeignModHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        dispatch:  &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let abi  = child_of_kind(node, "string_literal")
            .map(|n| ctx.text(n).trim_matches('"').to_string())
            .unwrap_or_else(|| "C".into());
        let name = format!("extern \"{abi}\"");
        let line = node.start_position().row as u32 + 1;
        let id   = ctx.make_id("ForeignMod", &name, line);
        let cn   = ctx.build(id.clone(), name, CodeNodeKind::Other("foreign_mod".into()), node);
        let oid  = ctx.push(cn, parent_id);

        if let Some(body) = node.child_by_field_name("body") {
            walk_item_list(body, ctx, Some(&oid), dispatch);
        }
    }
}
