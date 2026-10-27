//! Pure decisions about tool execution (design §7.3).
//!
//! Everything here is a pure function of its inputs: the sandbox decision and
//! the truncation policy are data, so they are tested without a process or a
//! filesystem.

pub mod sandbox;
pub mod truncate;

pub use sandbox::{is_self_service, Autonomy, BuiltinSandboxPolicy};
pub use truncate::{cap_all, cap_bytes, cap_lines, TruncationLimits, DEFAULT_LIMITS};
