//! Trace identifier propagation (design 7.5.1 `trace_id` pass-through).
//!
//! The bus mints a trace id when the caller does not carry one, and reuses a
//! well-formed inbound id so a plugin-to-plugin call keeps a single chain. The
//! id is echoed on the response headers for the caller to forward.

use ark_ports::HeaderMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub const TRACE_HEADER: &str = "x-ark-trace-id";

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Relaxed shape check: trace ids travel through third-party headers, so only
/// unambiguous characters are accepted.
pub fn is_valid_trace_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Mint a fresh id. Monotonic so two ids in the same process never collide.
pub fn mint_trace_id() -> String {
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("tr-{:x}-{:x}", std::process::id(), seq)
}

/// Reuse the inbound id when valid, otherwise mint one.
pub fn accept_or_mint(headers: &HeaderMap) -> String {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(TRACE_HEADER))
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| is_valid_trace_id(value))
        .unwrap_or_else(mint_trace_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_trace_id_is_reused() {
        let mut headers = HeaderMap::new();
        headers.insert(TRACE_HEADER.to_string(), "tr-abc-123".to_string());
        assert_eq!(accept_or_mint(&headers), "tr-abc-123");
    }

    #[test]
    fn malformed_trace_id_is_replaced() {
        let mut headers = HeaderMap::new();
        headers.insert(
            TRACE_HEADER.to_string(),
            "bad id with spaces\r\n".to_string(),
        );
        let id = accept_or_mint(&headers);
        assert!(is_valid_trace_id(&id), "{id} should be a valid minted id");
        assert_ne!(id, "bad id with spaces\r\n");
    }

    #[test]
    fn minted_ids_are_unique() {
        let a = mint_trace_id();
        let b = mint_trace_id();
        assert_ne!(a, b);
        assert!(is_valid_trace_id(&a) && is_valid_trace_id(&b));
    }
}
