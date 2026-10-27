//! Retry decision and backoff schedule (design 7.5.1).
//!
//! Both helpers are pure so the policy is testable without a socket.

use crate::config::RetryPolicy;

/// Methods that may be retried without an explicit idempotency key.
const IDEMPOTENT_METHODS: [&str; 5] = ["GET", "HEAD", "OPTIONS", "PUT", "DELETE"];

pub fn is_idempotent(method: &str) -> bool {
    let upper = method.to_ascii_uppercase();
    IDEMPOTENT_METHODS.contains(&upper.as_str())
}

/// Retryable transport statuses: provider-side unavailability, not our bug.
pub fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

/// Whether another attempt is allowed. `attempt` is 1-based.
pub fn should_retry(
    method: &str,
    status: Option<u16>,
    has_idempotency_key: bool,
    attempt: u32,
    policy: &RetryPolicy,
) -> bool {
    if attempt >= policy.max_attempts.max(1) {
        return false;
    }
    if !has_idempotency_key && !is_idempotent(method) {
        return false;
    }
    match status {
        // Transport error (no status): retry when the method is safe to repeat.
        None => true,
        Some(code) => retryable_status(code),
    }
}

/// Exponential backoff capped by `policy.max_ms`, deterministic for tests.
pub fn backoff_ms(attempt: u32, policy: &RetryPolicy) -> u64 {
    let exponent = attempt.saturating_sub(1).min(16);
    let raw = policy.base_ms.saturating_mul(1u64 << exponent);
    raw.min(policy.max_ms)
}

/// Spread retries so many plugins do not hammer the same host in lockstep.
pub fn jittered_backoff_ms(attempt: u32, policy: &RetryPolicy, jitter: u64) -> u64 {
    let base = backoff_ms(attempt, policy);
    if base == 0 {
        return 0;
    }
    base.saturating_sub(jitter % (base / 2 + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 3,
            base_ms: 100,
            max_ms: 250,
        }
    }

    #[test]
    fn post_is_not_retried_without_key() {
        assert!(!should_retry("POST", Some(503), false, 1, &policy()));
        assert!(should_retry("POST", Some(503), true, 1, &policy()));
    }

    #[test]
    fn retry_stops_at_max_attempts() {
        assert!(should_retry("GET", None, false, 1, &policy()));
        assert!(should_retry("GET", None, false, 2, &policy()));
        assert!(!should_retry("GET", None, false, 3, &policy()));
    }

    #[test]
    fn client_errors_are_not_retried() {
        assert!(!should_retry("GET", Some(404), false, 1, &policy()));
        assert!(!should_retry("GET", Some(401), false, 1, &policy()));
        assert!(should_retry("GET", Some(429), false, 1, &policy()));
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(backoff_ms(1, &policy()), 100);
        assert_eq!(backoff_ms(2, &policy()), 200);
        assert_eq!(backoff_ms(3, &policy()), 250);
        assert_eq!(backoff_ms(9, &policy()), 250);
    }

    #[test]
    fn jitter_never_exceeds_the_backoff() {
        for attempt in 1..5 {
            for jitter in 0..10 {
                assert!(
                    jittered_backoff_ms(attempt, &policy(), jitter)
                        <= backoff_ms(attempt, &policy())
                );
            }
        }
    }
}
