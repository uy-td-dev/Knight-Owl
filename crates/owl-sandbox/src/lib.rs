//! Sandbox crate — isolated code execution via Docker.
//!
//! Concrete implementations:
//! - [`docker::DockerSandbox`]: production, uses `bollard` (Docker API).
//! - [`local::LocalSandbox`]: fallback for environments without Docker;
//!   runs the command directly via `tokio::process`.
//!
//! Both implement `owl_protocol::sandbox::Sandbox`, which is the only surface
//! owl-brain ever touches (R-5).

#![forbid(unsafe_code)]

#[cfg(feature = "docker")]
pub mod docker;
pub mod local;

pub use local::LocalSandbox;

#[cfg(feature = "docker")]
pub use docker::DockerSandbox;
