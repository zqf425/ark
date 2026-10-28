//! ark-llm: model provider adapters, streaming parse and usage (design §7.2).
//!
//! # Layout
//!
//! ```text
//! config/    EndpointFormat routing decision, URL construction
//! provider/  one module per wire family + the single router
//! common/    SSE framing, stream assembly, usage, retry, repair
//! domain/    provider-neutral stream events
//! service/   the two ModelClient implementations (mock, HTTP)
//! ```
//!
//! # Why it is split this way
//!
//! `endpoint_format` decides which provider module handles a request *once*, in
//! [`provider::route`]. No function contains a per-format `if` chain, so adding a
//! family is a new module plus one match arm.
//!
//! Retry/backoff exists only in [`common::retry`], and `cache_hit_rate` is only
//! ever derived from a provider-native hit/miss pair ([`common::usage`]): it is
//! never inferred from `hit / prompt_tokens`.
//!
//! The stream parsers are free of transport code, so every one of them is unit
//! tested against literal SSE text fixtures without a network stack.

#![deny(clippy::all)]

pub mod common;
pub mod config;
pub mod domain;
pub mod provider;
pub mod service;

pub use common::{
    parse_tool_arguments, repair_messages, FailureKind, RetryPolicy, RetryState, SseEvent,
    SseParser, StreamAccumulator, UsageParser,
};
pub use config::{EndpointFormat, ProviderConfig};
pub use domain::{StreamEvent, ToolFragment};
pub use provider::route::Family;
pub use provider::StreamParser;
pub use service::{HttpModelClient, MockModelClient, ModelCatalogClient, Script, ScriptedCall};
