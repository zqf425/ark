//! Data-only tool domain: typed arguments, path containment, globs, schemas.
//!
//! Nothing here performs I/O, so the argument shapes, the containment rule and
//! the canonical schema fingerprints stay unit-testable without a filesystem.

pub mod args;
pub mod glob;
pub mod meta;
pub mod paths;
pub mod schema;

pub use args::{parse, BashArgs, EditArgs, FindArgs, GrepArgs, LsArgs, ReadArgs, WriteArgs};
pub use meta::{meta_of, names, ToolMeta, TOOL_METAS};
pub use paths::{require_simple_segment, resolve};
