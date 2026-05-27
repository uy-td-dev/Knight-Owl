//! Shared types, error enums, and events for the Knight-Owl framework.
//!
//! This crate has **zero** project-internal dependencies.
//! Every type derives `Debug + Clone + Serialize + Deserialize`.

#![forbid(unsafe_code)]

pub mod attachment;
pub mod code;
pub mod error;
pub mod events;
pub mod experience;
pub mod git;
pub mod graph;
pub mod ipc;
pub mod mcp;
pub mod memory;
pub mod orchestra;
pub mod pet;
pub mod sandbox;
pub mod schedule;
pub mod state;
pub mod tools;
pub mod vector;

pub use error::ProtocolError;
