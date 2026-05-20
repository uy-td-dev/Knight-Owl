//! All unit tests for the Rust AST parser.

use super::*;
use crate::ast::AstParser;
use owl_protocol::code::CodeNodeKind;

fn parse(src: &str) -> ParseResult {
    RustParser.parse("src/lib.rs", src.as_bytes(), 120).unwrap()
}

// ── Basic items ──────────────────────────────────────────────────────────────

#[test]
fn extracts_function() {
    let r = parse("pub fn hello() -> String { String::new() }");
    let n = r.nodes.iter().find(|n| n.name == "hello").unwrap();
    assert_eq!(n.kind, CodeNodeKind::Function);
    assert_eq!(n.visibility, "pub");
}

#[test]
fn extracts_async_function() {
    let r = parse("pub async fn fetch(url: &str) -> Result<(), ()> { Ok(()) }");
    let n = r.nodes.iter().find(|n| n.name == "fetch").unwrap();
    assert!(n.qualifiers.contains("async"), "expected async qualifier, got {:?}", n.qualifiers);
    assert_eq!(n.visibility, "pub");
}

#[test]
fn extracts_unsafe_fn() {
    let r = parse("pub unsafe fn raw_op() {}");
    let n = r.nodes.iter().find(|n| n.name == "raw_op").unwrap();
    assert!(n.qualifiers.contains("unsafe"));
}

#[test]
fn extracts_const_fn() {
    let r = parse("pub const fn const_op() -> u32 { 42 }");
    let n = r.nodes.iter().find(|n| n.name == "const_op").unwrap();
    assert!(n.qualifiers.contains("const"));
}

// ── Struct + fields ──────────────────────────────────────────────────────────

#[test]
fn extracts_struct_with_fields() {
    let r = parse(
        "pub struct Config { pub timeout: u64, pub(crate) retries: u32, name: String }",
    );
    let s = r.nodes.iter().find(|n| n.name == "Config").unwrap();
    assert_eq!(s.kind, CodeNodeKind::Struct);
    assert_eq!(s.visibility, "pub");

    let fields: Vec<_> = r.nodes.iter().filter(|n| n.kind == CodeNodeKind::Variable).collect();
    assert_eq!(fields.len(), 3, "expected 3 fields, got {}", fields.len());

    let timeout = fields.iter().find(|f| f.name == "timeout").unwrap();
    assert_eq!(timeout.visibility, "pub");
    let retries = fields.iter().find(|f| f.name == "retries").unwrap();
    assert_eq!(retries.visibility, "pub(crate)");
    let name_f = fields.iter().find(|f| f.name == "name").unwrap();
    assert_eq!(name_f.visibility, "");

    assert!(r.contains.iter().any(|(p, _)| p == &s.id));
}

#[test]
fn extracts_tuple_struct_fields() {
    let r = parse("pub struct Pair(pub i32, String);");
    let fields: Vec<_> = r.nodes.iter().filter(|n| n.kind == CodeNodeKind::Variable).collect();
    assert_eq!(fields.len(), 2, "expected 2 tuple fields");
    assert_eq!(fields[0].name, "_0");
    assert_eq!(fields[0].visibility, "pub");
    assert_eq!(fields[1].visibility, "");
}

// ── Enum ─────────────────────────────────────────────────────────────────────

#[test]
fn extracts_enum_variants() {
    let r = parse("pub enum State { Idle, Running, Done(u32) }");
    assert!(r.nodes.iter().any(|n| n.name == "State" && n.kind == CodeNodeKind::Enum));
    assert!(r.nodes.iter().any(|n| n.name == "Idle"));
    assert!(r.nodes.iter().any(|n| n.name == "Running"));
    assert!(r.nodes.iter().any(|n| n.name == "Done"));
}

// ── Impl + trait ─────────────────────────────────────────────────────────────

#[test]
fn extracts_impl_methods_as_method_kind() {
    let src = "impl Config { pub fn new() -> Self { Self::default() } }";
    let r = parse(src);
    let m = r.nodes.iter().find(|n| n.name == "new").unwrap();
    assert_eq!(m.kind, CodeNodeKind::Method);
    assert_eq!(m.visibility, "pub");
}

#[test]
fn duplicate_impl_blocks_get_distinct_ids() {
    let src = "impl Foo { fn a(&self) {} }\nimpl Foo { fn b(&self) {} }\n";
    let r = parse(src);
    let impls: Vec<_> = r.nodes.iter()
        .filter(|n| n.kind == CodeNodeKind::Impl)
        .collect();
    assert_eq!(impls.len(), 2, "expected 2 impl nodes");
    assert_ne!(impls[0].id, impls[1].id, "impl IDs must differ");
}

#[test]
fn extracts_trait_impl_name() {
    let src = "impl Default for Config { fn default() -> Self { Config {} } }";
    let r = parse(src);
    assert!(r.nodes.iter().any(|n| n.name.contains("Default") && n.name.contains("Config")));
}

#[test]
fn extracts_trait_signature_method() {
    let src = "pub trait Runner: Send {\n    async fn run(&self) -> Result<(), ()>;\n    fn name(&self) -> &'static str { \"runner\" }\n}\n";
    let r = parse(src);
    assert!(r.nodes.iter().any(|n| n.name == "Runner" && n.kind == CodeNodeKind::Trait));
    let run = r.nodes.iter().find(|n| n.name == "run").unwrap();
    assert_eq!(run.kind, CodeNodeKind::Method);
    assert!(run.qualifiers.contains("async"));
}

// ── Doc comments ─────────────────────────────────────────────────────────────

#[test]
fn extracts_doc_comments() {
    let src = "/// A configuration value.\n/// Loaded from config/app.toml.\npub struct Config {\n    /// Timeout in milliseconds.\n    pub timeout: u64,\n}\n";
    let r = parse(src);
    let s = r.nodes.iter().find(|n| n.name == "Config").unwrap();
    assert!(s.description.contains("configuration"), "got: {:?}", s.description);
    assert!(s.description.contains("config/app.toml"), "got: {:?}", s.description);

    let f = r.nodes.iter().find(|n| n.name == "timeout").unwrap();
    assert!(f.description.contains("Timeout"), "got: {:?}", f.description);
}

// ── use declarations ─────────────────────────────────────────────────────────

#[test]
fn flattens_use_list() {
    let r = parse("use std::sync::{Arc, Mutex, RwLock};");
    for name in ["Arc", "Mutex", "RwLock"] {
        assert!(
            r.import_refs.iter().any(|(_, p)| p.contains(name)),
            "missing import {name}",
        );
    }
}

#[test]
fn flattens_nested_use() {
    let r = parse("use std::{collections::{HashMap, BTreeMap}, sync::Arc};");
    for name in ["HashMap", "BTreeMap", "Arc"] {
        assert!(
            r.import_refs.iter().any(|(_, p)| p.contains(name)),
            "missing import {name}",
        );
    }
}

#[test]
fn flattens_use_glob() {
    let r = parse("use crate::prelude::*;");
    assert!(r.import_refs.iter().any(|(_, p)| p.ends_with("::*") || p.ends_with('*')));
}

#[test]
fn flattens_use_as() {
    let r = parse("use std::io::Error as IoError;");
    assert!(r.import_refs.iter().any(|(_, p)| p.contains("as IoError")));
}

// ── CALLS ────────────────────────────────────────────────────────────────────

#[test]
fn detects_direct_calls() {
    let r = parse("fn a() {} fn b() { a(); }");
    assert!(r.call_refs.iter().any(|(_, c)| c == "a"));
}

#[test]
fn detects_method_calls() {
    let r = parse("fn f() { let v: Vec<i32> = Vec::new(); v.len(); v.push(1); }");
    assert!(r.call_refs.iter().any(|(_, c)| c.contains("new")));
    assert!(r.call_refs.iter().any(|(_, c)| c == "len"));
    assert!(r.call_refs.iter().any(|(_, c)| c == "push"));
}

#[test]
fn detects_macro_calls() {
    let r = parse("fn f() { println!(\"hi\"); vec![1]; panic!(\"no\"); }");
    assert!(r.call_refs.iter().any(|(_, c)| c == "println!"));
    assert!(r.call_refs.iter().any(|(_, c)| c == "vec!"));
    assert!(r.call_refs.iter().any(|(_, c)| c == "panic!"));
}

#[test]
fn calls_do_not_escape_nested_fn() {
    let src = "fn outer() {\n    fn inner() { helper(); }\n}\nfn helper() {}\n";
    let r = parse(src);
    let outer = r.nodes.iter().find(|n| n.name == "outer").unwrap();
    let outer_calls: Vec<_> = r.call_refs.iter()
        .filter(|(caller, _)| caller == &outer.id)
        .collect();
    assert!(
        outer_calls.iter().all(|(_, c)| c != "helper"),
        "helper call must not be attributed to outer",
    );
}

// ── Modules ───────────────────────────────────────────────────────────────────

#[test]
fn inline_mod_children_are_contained() {
    let src = "mod config {\n    pub struct Config { pub x: i32 }\n    pub fn load() -> Config { Config { x: 0 } }\n}\n";
    let r = parse(src);
    let m = r.nodes.iter().find(|n| n.name == "config").unwrap();
    assert!(r.contains.iter().any(|(p, _)| p == &m.id), "mod must have CONTAINS edges");
}

// ── Misc ──────────────────────────────────────────────────────────────────────

#[test]
fn extracts_const_and_static() {
    let r = parse("pub const MAX: u32 = 3;\npub static APP: &str = \"owl\";");
    assert!(r.nodes.iter().any(|n| n.name == "MAX" && n.kind == CodeNodeKind::Constant));
    assert!(r.nodes.iter().any(|n| n.name == "APP" && n.kind == CodeNodeKind::Variable));
}

#[test]
fn extracts_type_alias() {
    let r = parse("pub type Result<T> = std::result::Result<T, MyError>;");
    assert!(r.nodes.iter().any(|n| n.name == "Result" && n.kind == CodeNodeKind::TypeAlias));
}

#[test]
fn extracts_macro_def() {
    let r = parse("macro_rules! my_mac { () => {} }");
    assert!(r.nodes.iter().any(|n| n.name == "my_mac"));
}

/// Self-hosting: parse mod.rs without errors and produce non-empty output.
#[test]
fn parses_this_file_without_errors() {
    let src = include_str!("mod.rs");
    let r = RustParser.parse("src/ast/rust/mod.rs", src.as_bytes(), 200).unwrap();
    assert!(!r.nodes.is_empty());
    for n in &r.nodes {
        assert!(!n.id.is_empty(), "empty id for node {:?}", n.name);
        assert!(!n.file_path.is_empty());
    }
}
