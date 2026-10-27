//! Tool services: the registry and the runner the reactor calls.
//!
//! `RegistryToolRunner` is the seam between `ark-agent`'s `ToolRunner` trait and
//! the registry: the agent gets one object, and the sandbox decision is computed
//! once per call before dispatch.

pub mod registry;
pub mod runner;

pub use registry::ToolRegistry;
pub use runner::RegistryToolRunner;
