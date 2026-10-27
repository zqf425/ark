//! ark-http: Base capability: outbound HTTP with allowlist/credential injection (design 7.5.1).
//!
//! Layer folders follow the QDC mapping in design 5.1:
//!
//! * [`config`] - policy values (timeouts, retry, rate limit, body caps).
//! * [`domain`] - pure rules: allowlist matching, retry decisions, token
//!   buckets, header guards, redirect resolution, trace ids, fingerprints.
//!   No I/O, so every rule is unit-testable.
//! * [`service`] - the one `reqwest::Client`, the request preparation pipeline
//!   and [`HttpBus`], which implements [`ark_ports::HttpCapability`].
//!
//! ## Why plugins cannot bypass this crate
//!
//! A plugin gets `Arc<dyn HttpCapability>` and nothing else. It cannot build a
//! `reqwest::Client`, cannot set `User-Agent`/`Host`/`Authorization` headers and
//! cannot reach a host outside its manifest allowlist. Credentials are
//! referenced by name; only this crate resolves them, only for the owning
//! plugin, and only the `name@hash-prefix` label is ever logged.

#![deny(clippy::all)]

pub mod config;
pub mod domain;
pub mod service;

pub use config::{HttpPolicy, RateLimitPolicy, RetryPolicy};
pub use domain::{
    fingerprint, header_digest, host_of, is_reserved_header, is_valid_trace_id, mint_trace_id,
    next_hop, parse_reference, parse_target, redact_url, sanitize_headers, user_agent,
    validate_plugin_id, CredentialRef, Hop, HostAllowlist, HostPattern, HttpDenialReason,
    HttpDiagnostic, HttpDiagnosticSink, MemoryDiagnostics,
};
pub use service::{
    denial_reason, prepare, CredentialVault, EgressGrant, EgressRegistry, ExecuteError, Executor,
    HttpBus, HttpBusBuilder, Prepared,
};
