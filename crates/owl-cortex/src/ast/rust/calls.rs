//! Call-graph extraction — collect_calls + resolve_callee.

use tree_sitter::Node;

use super::context::WalkCtx;

/// DFS inside a node — collect `call_expression` and `macro_invocation`.
/// Does NOT descend into nested `function_item` (distinct scope).
pub fn collect_calls<'src>(ctx: &mut WalkCtx<'src>, node: Node<'src>, caller_id: &str) {
    let mut stack = vec![node];
    while let Some(cur) = stack.pop() {
        match cur.kind() {
            "call_expression" => {
                if let Some(func) = cur.child_by_field_name("function") {
                    let callee = resolve_callee(ctx, func);
                    ctx.result.call_refs.push((caller_id.to_string(), callee));
                }
            }
            "macro_invocation" => {
                if let Some(mac) = cur.child_by_field_name("macro") {
                    let name = format!("{}!", ctx.text(mac));
                    ctx.result.call_refs.push((caller_id.to_string(), name));
                }
            }
            // Stop at nested function/closure boundaries.
            "function_item" | "closure_expression" => continue,
            _ => {}
        }
        let mut c = cur.walk();
        stack.extend(cur.named_children(&mut c));
    }
}

/// Resolve the callee name from the `function` field of a `call_expression`.
pub fn resolve_callee<'src>(ctx: &WalkCtx<'src>, func: Node<'src>) -> String {
    match func.kind() {
        "identifier" => ctx.text(func).to_string(),
        "scoped_identifier" => {
            let path = func.child_by_field_name("path").map(|n| ctx.text(n)).unwrap_or("");
            let name = func.child_by_field_name("name").map(|n| ctx.text(n)).unwrap_or("");
            if path.is_empty() { name.into() } else { format!("{path}::{name}") }
        }
        "field_expression" => {
            func.child_by_field_name("field")
                .map(|n| ctx.text(n).to_string())
                .unwrap_or_else(|| ctx.text(func).to_string())
        }
        "generic_function" => {
            func.child_by_field_name("function")
                .map(|inner| resolve_callee(ctx, inner))
                .unwrap_or_else(|| ctx.text(func).to_string())
        }
        _ => ctx.text(func).to_string(),
    }
}
