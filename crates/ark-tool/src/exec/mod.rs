//! Tool execution layer: the seven builtins plus their shared plumbing.
//!
//! `common` holds the helper every tool shares (path resolution, error-to-output
//! conversion), and `tools` holds one module per behaviour group. Nothing here
//! decides policy: the sandbox decision already arrived in `ToolContext`.

pub mod common;
pub mod tools;

#[cfg(test)]
pub(crate) mod testsupport;
