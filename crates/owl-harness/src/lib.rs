//! The Training Grounds — mocks and evaluators for owl-brain tests.
//!
//! This crate MUST only appear as `[dev-dependencies]` in other crates.

pub mod evaluator;
pub mod mock_engine;
pub mod mock_tools;
pub mod recorder;

pub use evaluator::Evaluator;
pub use mock_engine::MockEngine;
pub use mock_tools::MockToolExecutor;
pub use recorder::{Trace, TraceDiff};
