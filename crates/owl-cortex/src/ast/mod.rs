//! Language-specific AST parsers.
//!
//! Each language is a separate module gated on the matching Cargo feature.
//! All parsers implement the same `AstParser` trait so `Ingestor` stays
//! language-agnostic.

use owl_protocol::code::CodeNode;

use crate::CortexError;

/// Result of parsing one source file.
#[derive(Debug, Default)]
pub struct ParseResult {
    /// All code elements extracted from the file.
    pub nodes: Vec<CodeNode>,
    /// Pairs of (caller node id, callee name) — unresolved at this stage.
    pub call_refs: Vec<(String, String)>,
    /// Pairs of (importer node id, import path string).
    pub import_refs: Vec<(String, String)>,
    /// Pairs of (parent node id, child node id) — containment.
    pub contains: Vec<(String, String)>,
}

/// Pluggable AST parser for a single language.
pub trait AstParser: Send + Sync {
    /// Language identifier, e.g. `"rust"`.
    fn lang(&self) -> &'static str;

    /// File extension(s) handled, e.g. `["rs"]`.
    fn extensions(&self) -> &'static [&'static str];

    /// Parse `source` as if it lives at `file_path`.
    fn parse(&self, file_path: &str, source: &[u8], preview_bytes: usize)
        -> Result<ParseResult, CortexError>;
}

#[cfg(feature = "lang-rust")]
pub mod rust;

#[cfg(feature = "lang-ts")]
pub mod typescript;

#[cfg(feature = "lang-py")]
pub mod python;

/// Return a parser for the given file extension, if one is registered.
pub fn parser_for_ext(ext: &str) -> Option<Box<dyn AstParser>> {
    #[cfg(feature = "lang-rust")]
    if rust::RustParser::EXTENSIONS.contains(&ext) {
        return Some(Box::new(rust::RustParser));
    }
    #[cfg(feature = "lang-ts")]
    if typescript::TypeScriptParser::EXTENSIONS.contains(&ext) {
        return Some(Box::new(typescript::TypeScriptParser));
    }
    #[cfg(feature = "lang-py")]
    if python::PythonParser::EXTENSIONS.contains(&ext) {
        return Some(Box::new(python::PythonParser));
    }
    let _ = ext;
    None
}
