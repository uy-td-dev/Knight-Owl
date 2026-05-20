//! The Arsenal — every native tool follows the same contract.
//!
//! Tools are pure functions over I/O with no internal state.
//! Registry is the single source of all registered tools.

#![forbid(unsafe_code)]

pub mod error;
pub mod registry;
pub mod tools;
pub mod traits;

pub use error::ArmoryError;
pub use registry::build_registry;
pub use traits::NativeTool;
