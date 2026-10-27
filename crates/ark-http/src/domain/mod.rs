//! Pure rules of the outbound path: allowlist, retry, rate limit, guards.

pub mod allowlist;
pub mod credential_ref;
pub mod diagnostics;
pub mod fingerprint;
pub mod rate_limit;
pub mod redirect;
pub mod request_guard;
pub mod retry;
pub mod trace;

pub use allowlist::{HostAllowlist, HostPattern};
pub use credential_ref::{parse_reference, CredentialRef};
pub use diagnostics::{HttpDenialReason, HttpDiagnostic, HttpDiagnosticSink, MemoryDiagnostics};
pub use fingerprint::{fingerprint, sha256};
pub use rate_limit::{RateLimiter, TokenBucket};
pub use redirect::{next_hop, redirect_budget, Hop};
pub use request_guard::{
    header_digest, host_of, is_reserved_header, parse_target, redact_url, sanitize_headers,
    user_agent, validate_plugin_id, PLUGIN_HEADER, TRACE_HEADER,
};
pub use retry::{backoff_ms, is_idempotent, jittered_backoff_ms, retryable_status, should_retry};
pub use trace::{accept_or_mint, is_valid_trace_id, mint_trace_id};
