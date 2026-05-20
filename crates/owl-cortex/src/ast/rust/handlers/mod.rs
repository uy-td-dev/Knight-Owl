//! Handler registry — assembles all `NodeHandler` implementations.
//!
//! To add a new Rust AST node kind:
//!   1. Create `handlers/<name>.rs` and `impl NodeHandler`.
//!   2. Add one `r.register(...)` line in `build_registry()` below.
//!   3. No other files change (R-2: open/closed).

pub mod enum_;
pub mod extern_crate;
pub mod foreign_mod;
pub mod function;
pub mod impl_;
pub mod macro_def;
pub mod mod_;
pub mod simple;
pub mod struct_;
pub mod trait_;
pub mod use_;

use owl_protocol::code::CodeNodeKind;

use crate::ast::rust::registry::HandlerRegistry;

use self::enum_::EnumHandler;
use self::extern_crate::ExternCrateHandler;
use self::foreign_mod::ForeignModHandler;
use self::function::FunctionHandler;
use self::impl_::ImplHandler;
use self::macro_def::MacroDefHandler;
use self::mod_::ModHandler;
use self::simple::SimpleHandler;
use self::struct_::StructHandler;
use self::trait_::TraitHandler;
use self::use_::UseHandler;

/// Construct the registry with all known Rust AST node handlers.
pub fn build_registry() -> HandlerRegistry {
    let mut r = HandlerRegistry::new();
    r.register(&["function_item", "function_signature_item"], FunctionHandler);
    r.register(&["struct_item", "union_item"],               StructHandler);
    r.register(&["enum_item"],                               EnumHandler);
    r.register(&["trait_item"],                              TraitHandler);
    r.register(&["impl_item"],                               ImplHandler);
    r.register(&["mod_item"],                                ModHandler);
    r.register(&["const_item"],   SimpleHandler(CodeNodeKind::Constant));
    r.register(&["static_item"],  SimpleHandler(CodeNodeKind::Variable));
    r.register(&["type_item", "associated_type"], SimpleHandler(CodeNodeKind::TypeAlias));
    r.register(&["use_declaration"],          UseHandler);
    r.register(&["macro_definition"],         MacroDefHandler);
    r.register(&["extern_crate_declaration"], ExternCrateHandler);
    r.register(&["foreign_mod_item"],         ForeignModHandler);
    r
}
