//! Token buckets keyed by `plugin_id + host` (design 7.5.1).
//!
//! One misbehaving plugin must not exhaust the shared egress, so the bucket
//! dimension includes the plugin id and is never merged.

use crate::config::RateLimitPolicy;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// One bucket. Time is injected so tests do not sleep.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    capacity: f64,
    refill_per_sec: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, refill_per_sec: f64, now: Instant) -> Self {
        Self {
            capacity,
            refill_per_sec,
            tokens: capacity,
            last: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        if now <= self.last {
            return;
        }
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last = now;
    }

    /// Consume one token; `false` means the caller is over quota.
    pub fn try_acquire(&mut self, now: Instant) -> bool {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub fn available(&self) -> f64 {
        self.tokens
    }

    /// Hint for how long the caller could wait for the next token.
    pub fn retry_after(&self) -> Duration {
        if self.tokens >= 1.0 || self.refill_per_sec <= 0.0 {
            return Duration::ZERO;
        }
        let missing = 1.0 - self.tokens;
        Duration::from_secs_f64(missing / self.refill_per_sec)
    }
}

/// Bucket map for the whole bus.
#[derive(Debug)]
pub struct RateLimiter {
    policy: RateLimitPolicy,
    buckets: BTreeMap<(String, String), TokenBucket>,
}

impl RateLimiter {
    pub fn new(policy: RateLimitPolicy) -> Self {
        Self {
            policy,
            buckets: BTreeMap::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.policy.enabled()
    }

    /// Check and consume one token for `(plugin_id, host)`.
    pub fn acquire(&mut self, plugin_id: &str, host: &str, now: Instant) -> bool {
        if !self.policy.enabled() {
            return true;
        }
        let key = (plugin_id.to_string(), host.to_ascii_lowercase());
        let policy = self.policy.clone();
        let bucket = self
            .buckets
            .entry(key)
            .or_insert_with(|| TokenBucket::new(policy.capacity, policy.refill_per_sec, now));
        bucket.try_acquire(now)
    }

    pub fn retry_after(&self, plugin_id: &str, host: &str) -> Duration {
        let key = (plugin_id.to_string(), host.to_ascii_lowercase());
        self.buckets
            .get(&key)
            .map(TokenBucket::retry_after)
            .unwrap_or(Duration::ZERO)
    }

    pub fn tracked(&self) -> usize {
        self.buckets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(capacity: f64, refill: f64) -> RateLimiter {
        RateLimiter::new(RateLimitPolicy {
            capacity,
            refill_per_sec: refill,
            per_host_capacity: 0.0,
            per_host_refill_per_sec: 0.0,
        })
    }

    #[test]
    fn bucket_drains_then_refills() {
        let now = Instant::now();
        let mut limiter = limiter(2.0, 1.0);
        assert!(limiter.acquire("p1", "api.example.com", now));
        assert!(limiter.acquire("p1", "api.example.com", now));
        assert!(!limiter.acquire("p1", "api.example.com", now));
        let later = now + Duration::from_secs(1);
        assert!(limiter.acquire("p1", "api.example.com", later));
    }

    #[test]
    fn plugins_do_not_share_a_bucket() {
        let now = Instant::now();
        let mut limiter = limiter(1.0, 0.0);
        assert!(limiter.acquire("p1", "api.example.com", now));
        assert!(!limiter.acquire("p1", "api.example.com", now));
        // Same host, different plugin: still allowed.
        assert!(limiter.acquire("p2", "api.example.com", now));
        assert_eq!(limiter.tracked(), 2);
    }

    #[test]
    fn hosts_do_not_share_a_bucket() {
        let now = Instant::now();
        let mut limiter = limiter(1.0, 0.0);
        assert!(limiter.acquire("p1", "a.example.com", now));
        assert!(limiter.acquire("p1", "b.example.com", now));
    }

    #[test]
    fn disabled_limiter_always_allows() {
        let mut limiter = limiter(0.0, 0.0);
        for _ in 0..100 {
            assert!(limiter.acquire("p1", "api.example.com", Instant::now()));
        }
        assert_eq!(limiter.tracked(), 0);
    }
}
