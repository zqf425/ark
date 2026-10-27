//! ark-tool: Tool registry, builtin tools, sandbox policy (design §7.3).
//!
//! Layer folders follow the QDC mapping in design §5.1:
//!
//! ```text
//! domain/   data only: typed args, path containment, globs, canonical schemas
//! policy/   pure decisions: sandbox autonomy, output truncation
//! service/  ToolRegistry (schema fingerprint cache) + the runner adapter
//! exec/     the seven builtin tools and their shared plumbing
//! ```
//!
//! Two invariants drive the layout:
//!
//! 1. the sandbox decision is computed **once** before dispatch and handed to the
//!    tool inside `ToolContext`, so no tool re-derives its own rules;
//! 2. every filesystem path goes through `ark_common::path::normalize` and is
//!    then checked against the workspace root, so an escape is impossible by
//!    construction rather than by each tool remembering to check.

#![deny(clippy::all)]

pub mod domain;
pub mod exec;
pub mod policy;
pub mod service;

pub use domain::{schema, ToolMeta, TOOL_METAS};
pub use exec::tools::{
    builtins, delegation_tools, BashTool, DelegateTaskTool, EditTool, FindTool, GrepTool, LsTool,
    ReadTool, WriteTool,
};
pub use policy::{
    cap_bytes, cap_lines, is_self_service, Autonomy, BuiltinSandboxPolicy, TruncationLimits,
    DEFAULT_LIMITS,
};
pub use service::{RegistryToolRunner, ToolRegistry};
