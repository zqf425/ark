//! Service layer: installing the global tracing subscriber.

pub mod tracing_init;

pub use tracing_init::{init_tracing, try_init_tracing, TracingGuard};
