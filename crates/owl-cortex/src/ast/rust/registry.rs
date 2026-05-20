//! `NodeHandler` trait + `HandlerRegistry` — the Strategy + Registry pattern core.
//!
//! Adding a new Rust AST node kind:
//!   1. Create `handlers/<name>.rs` and implement `NodeHandler`.
//!   2. Add one `registry.register(...)` line in `handlers/mod.rs::build_registry()`.
//!   3. No other files change (R-2: open/closed).

use std::collections::HashMap;
use std::sync::Arc;

use tree_sitter::Node;

use super::context::WalkCtx;

/// Strategy for extracting one category of Rust AST node.
///
/// R-4: exactly one method — minimum viable interface.
pub trait NodeHandler: Send + Sync {
    /// Extract items from `node` and push them into `ctx`.
    ///
    /// `parent_id` is the UUIDv5 of the enclosing item (`None` at file root).
    /// `dispatch` lets recursive handlers (impl, trait, mod, foreign_mod) schedule
    /// child nodes without needing a reference to the registry.
    ///
    /// The explicit `'src` lifetime ties `Node`, `WalkCtx`, and the `dispatch`
    /// closure together so Rust's borrow checker can verify the tree-sitter
    /// source borrows don't escape.
    fn handle<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        dispatch:  &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    );
}

/// Maps tree-sitter node kind strings to their handler.
pub struct HandlerRegistry {
    handlers: HashMap<&'static str, Arc<dyn NodeHandler>>,
}

impl HandlerRegistry {
    pub fn new() -> Self {
        Self { handlers: HashMap::new() }
    }

    /// Register a handler under one or more node kind strings.
    ///
    /// Uses `Arc` so the same handler instance can be inserted under multiple keys
    /// (e.g. `StructHandler` handles both `struct_item` and `union_item`).
    pub fn register(&mut self, kinds: &[&'static str], handler: impl NodeHandler + 'static) {
        let h: Arc<dyn NodeHandler> = Arc::new(handler);
        for kind in kinds {
            self.handlers.insert(kind, Arc::clone(&h));
        }
    }

    /// Dispatch a node to its registered handler, if any.
    ///
    /// Unknown node kinds (comments, attributes, punctuation) are silently skipped.
    pub fn dispatch<'src>(
        &self,
        node:      Node<'src>,
        ctx:       &mut WalkCtx<'src>,
        parent_id: Option<&str>,
        dispatch:  &dyn Fn(Node<'src>, &mut WalkCtx<'src>, Option<&str>),
    ) {
        if let Some(handler) = self.handlers.get(node.kind()) {
            handler.handle(node, ctx, parent_id, dispatch);
        }
    }
}

/// Top-level dispatch entry point — wraps the registry in a recursive closure.
///
/// A named function (not a closure) so the recursive self-reference compiles.
pub fn dispatch_node<'src>(
    registry:  &HandlerRegistry,
    node:      Node<'src>,
    ctx:       &mut WalkCtx<'src>,
    parent_id: Option<&str>,
) {
    registry.dispatch(node, ctx, parent_id, &|n, c, p| dispatch_node(registry, n, c, p));
}
