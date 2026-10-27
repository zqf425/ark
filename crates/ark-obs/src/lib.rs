//! ark-obs: observability - tracing wiring and usage metering (design 7.11).
//!
//! Scope for v1 is deliberately small and honest:
//!
//! * [`init_tracing`] installs a global subscriber exactly once, so a second
//!   call from a test or a plugin cannot panic the process.
//! * [`UsageRecorder`] is the single accumulator for provider-reported usage.
//!   It never derives a cache hit rate when the provider did not report both
//!   hit and miss counters (see [`ark_common::Usage::rate`]).
//!
//! Everything else the design lists (metric export, span sinks, audit) is the
//! caller's responsibility through the `ark-ports` service traits; this crate
//! does not grow a second, competing registry.

#![deny(clippy::all)]

pub mod config;
pub mod domain;
pub mod service;

pub use config::{LogFormat, TracingConfig};
pub use domain::{UsageRecorder, UsageTotals};
pub use service::tracing_init::{init_tracing, try_init_tracing, TracingGuard};
