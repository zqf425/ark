//! Provider modules. `endpointFormat` routes to exactly one of these (design
//! §7.2): the router is the only place that switches on the format.
//!
//! Each module owns its wire family completely — request body, headers, frame
//! parsing and usage extraction — and is free of transport code, so every parser
//! is unit-testable from literal SSE text without a network stack.

pub mod anthropic;
pub mod openai_chat;
pub mod openai_responses;
pub mod route;

use crate::common::sse::SseEvent;
use crate::domain::event::StreamEvent;

/// Per-family frame parser. Stateful only for provider bookkeeping such as the
/// Anthropic content-block index or the Responses item id.
pub trait StreamParser: Send {
    /// Parse one SSE frame into zero or more events.
    fn parse(&mut self, event: &SseEvent) -> Vec<StreamEvent>;

    /// Flush anything held back until the stream ended.
    fn finish(&mut self) -> Vec<StreamEvent> {
        Vec::new()
    }
}
