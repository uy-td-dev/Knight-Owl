//! `UseHandler` — use_declaration (fully flattened).

use tree_sitter::Node;

use owl_protocol::code::{CodeNode, CodeNodeKind};

use crate::ast::rust::context::WalkCtx;
use crate::ast::rust::registry::NodeHandler;
use crate::ast::rust::util::flatten_use_tree;

pub struct UseHandler;

impl NodeHandler for UseHandler {
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        _dispatch: &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        let line = node.start_position().row as u32 + 1;
        let Some(arg) = node.child_by_field_name("argument") else { return };

        let mut paths = Vec::new();
        flatten_use_tree(arg, ctx.source, "", &mut paths);

        for path in paths {
            let id = ctx.make_id("Import", &path, line);
            let cn = CodeNode {
                id:          id.clone(),
                file_path:   ctx.file_path.to_string(),
                name:        path.clone(),
                kind:        CodeNodeKind::Import,
                start_line:  line,
                end_line:    node.end_position().row as u32 + 1,
                preview:     format!("use {path}"),
                visibility:  ctx.visibility_of(node),
                qualifiers:  String::new(),
                description: ctx.doc_comment_above(node),
            };
            ctx.result.import_refs.push((id.clone(), path));
            ctx.push(cn, parent_id);
        }
    }
}
