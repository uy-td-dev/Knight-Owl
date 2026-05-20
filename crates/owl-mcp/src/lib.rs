//! The Squire — connects to external MCP servers transparently.
//!
//! `client.rs` sees only the `McpTransport` trait — never concrete transport types.

#![forbid(unsafe_code)]

pub mod client;
pub mod connector;
pub mod error;
pub mod transport;

pub use client::McpClient;
pub use connector::connect as connect_server;
pub use error::McpClientError;
