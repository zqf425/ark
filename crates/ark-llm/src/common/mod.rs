//! Shared adapter machinery: SSE framing, usage parsing, retry, repair.
//!
//! Every provider module builds on these, so wire-format knowledge that is not
//! family-specific lives here exactly once (design §7.2).

pub mod repair;
pub mod retry;
pub mod sse;
pub mod stream;
pub mod usage;

pub use repair::{parse_tool_arguments, repair_messages};
pub use retry::{FailureKind, RetryPolicy, RetryState};
pub use sse::{SseEvent, SseParser};
pub use stream::StreamAccumulator;
pub use usage::UsageParser;
