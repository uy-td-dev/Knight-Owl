//! App-layer tool bridges — structs that implement [`NativeTool`] but depend
//! on crates that cannot import each other (R-13).

pub mod mcp_proxy;
pub mod search_code;
