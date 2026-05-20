//! `EnumHandler` — enum_item.

use tree_sitter::Node;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;

pub struct EnumHandler;

impl NodeHandler for EnumHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        _dispatch: &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let name   = ctx.field_text(node, "name").unwrap_or("<anonymous>").to_string();
        let line   = node.start_position().row as u32 + 1;
        let id     = ctx.make_id("Enum", &name, line);
        let cn     = ctx.build(id.clone(), name.clone(), CodeNodeKind::Enum, node);
        let own_id = ctx.push(cn, parent_id);

        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for variant in body.named_children(&mut cursor) {
                if variant.kind() != "enum_variant" { continue; }
                let vname = ctx.field_text(variant, "name").unwrap_or("<variant>").to_string();
                let vline = variant.start_position().row as u32 + 1;
                let vid   = ctx.make_id("Variant", &format!("{name}::{vname}"), vline);
                let vcn   = ctx.build(vid, vname, CodeNodeKind::Other("variant".into()), variant);
                ctx.push(vcn, Some(&own_id));
            }
        }
    }
}
