//! Outbound HTTP policy configuration (design 7.5.1).
//!
//! Every knob the bus owns (proxy, TLS verification, timeout, redirect,
//! retry, rate limit, body cap) lives here so a plugin never re-derives it.

pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_MAX_TIMEOUT_MS: u64 = 120_000;
pub const DEFAULT_MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Retry budget shared by every plugin; a plugin cannot raise it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts including the first one. `1` disables retries.
    pub max_attempts: u32,
    pub base_ms: u64,
    pub max_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_ms: 200,
            max_ms: 5_000,
        }
    }
}

/// Token bucket governance per `plugin_id + host` (design 7.5.1).
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitPolicy {
    pub capacity: f64,
    pub refill_per_sec: f64,
    /// `0` disables the limiter entirely.
    pub per_host_capacity: f64,
    pub per_host_refill_per_sec: f64,
}

impl Default for RateLimitPolicy {
    fn default() -> Self {
        Self {
            capacity: 60.0,
            refill_per_sec: 30.0,
            per_host_capacity: 0.0,
            per_host_refill_per_sec: 0.0,
        }
    }
}

impl RateLimitPolicy {
    /// A limiter is active as soon as it has capacity. A zero refill rate is a
    /// hard quota (burst then block), not a disabled limiter.
    pub fn enabled(&self) -> bool {
        self.capacity > 0.0
    }
}

/// Whole-bus policy. Values are clamped, never trusted from a plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpPolicy {
    pub default_timeout_ms: u64,
    pub max_timeout_ms: u64,
    /// Plain `http://` is rejected unless the host opts in (loopback tests).
    pub allow_insecure_http: bool,
    pub max_body_bytes: usize,
    pub follow_redirects: bool,
    pub max_redirects: u8,
    pub retry: RetryPolicy,
    pub rate: RateLimitPolicy,
    /// Upper bound for one response body header block.
    pub max_headers: usize,
    pub proxy: Option<String>,
    pub accept_invalid_certs: bool,
}

impl Default for HttpPolicy {
    fn default() -> Self {
        Self {
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            max_timeout_ms: DEFAULT_MAX_TIMEOUT_MS,
            allow_insecure_http: false,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            follow_redirects: true,
            max_redirects: 5,
            retry: RetryPolicy::default(),
            rate: RateLimitPolicy::default(),
            max_headers: 100,
            proxy: None,
            accept_invalid_certs: false,
        }
    }
}

impl HttpPolicy {
    /// Loopback-friendly policy used by tests and local plugin development.
    pub fn loopback_permissive() -> Self {
        Self {
            allow_insecure_http: true,
            rate: RateLimitPolicy {
                capacity: 0.0,
                refill_per_sec: 0.0,
                ..RateLimitPolicy::default()
            },
            retry: RetryPolicy {
                max_attempts: 1,
                ..RetryPolicy::default()
            },
            ..Self::default()
        }
    }

    /// Effective timeout: request value clamped into `[1, max_timeout_ms]`.
    pub fn timeout_ms(&self, requested: Option<u64>) -> u64 {
        let requested = requested.unwrap_or(self.default_timeout_ms);
        requested.clamp(1, self.max_timeout_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_timeout_is_clamped() {
        let policy = HttpPolicy {
            max_timeout_ms: 5_000,
            ..HttpPolicy::default()
        };
        assert_eq!(policy.timeout_ms(None), DEFAULT_TIMEOUT_MS.min(5_000));
        assert_eq!(policy.timeout_ms(Some(0)), 1);
        assert_eq!(policy.timeout_ms(Some(999_999)), 5_000);
    }

    #[test]
    fn rate_limit_disabled_switch() {
        let off = RateLimitPolicy {
            capacity: 0.0,
            refill_per_sec: 0.0,
            ..RateLimitPolicy::default()
        };
        assert!(!off.enabled());
        assert!(RateLimitPolicy::default().enabled());
        // Zero refill is a hard quota, not a disable switch.
        let quota = RateLimitPolicy {
            capacity: 1.0,
            refill_per_sec: 0.0,
            ..RateLimitPolicy::default()
        };
        assert!(quota.enabled());
    }
}
