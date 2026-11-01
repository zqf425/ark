//! Retry policy — the single place exponential backoff exists (design §7.2).
//!
//! Two properties matter for testability: the delay computation is pure, and the
//! decision of *whether* to retry is a pure function of the classification. That
//! keeps the HTTP client free of `if status == 429 || status >= 500 || …` chains
//! and keeps every provider on the same schedule.

use std::time::Duration;

/// How a failed attempt should be classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Connection reset, timeout, truncated stream: worth another attempt.
    Transient,
    /// 429 / rate limited: worth another attempt, but later.
    RateLimited,
    /// 4xx that repeating cannot fix (bad request, auth, unknown model).
    Permanent,
}

impl FailureKind {
    /// Classify an HTTP status.
    pub fn from_status(status: u16) -> Self {
        match status {
            408 | 409 | 425 | 500..=599 => FailureKind::Transient,
            429 => FailureKind::RateLimited,
            _ => FailureKind::Permanent,
        }
    }

    pub fn retryable(self) -> bool {
        !matches!(self, FailureKind::Permanent)
    }
}

/// Exponential backoff with a cap and a rate-limit floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base: Duration,
    pub max_delay: Duration,
    /// Minimum delay for `RateLimited`, so a 429 does not hammer the provider.
    pub rate_limit_floor: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        // Matches smart-ark's compat-model-client: initial call + 3 retries
        // (their competitor survey landed on 3 as the middle ground between
        // dsh's 2 and gemini-cli's stall-prone 10). The 30s cap keeps a
        // provider-supplied Retry-After from stalling a turn for minutes.
        Self {
            max_attempts: 4,
            base: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            rate_limit_floor: Duration::from_secs(1),
        }
    }
}

impl RetryPolicy {
    /// A policy with no retries, used by tests and by callers that manage their
    /// own retry budget.
    pub fn none() -> Self {
        Self {
            max_attempts: 1,
            ..Self::default()
        }
    }

    /// Delay before attempt `attempt` (0-based: 0 is the delay after the first
    /// failure). `base × 2^attempt`, capped, floored for rate limiting.
    pub fn delay(&self, attempt: u32, kind: FailureKind) -> Duration {
        let exponent = attempt.min(16);
        let scaled = self.base.saturating_mul(1u32 << exponent.min(16));
        let capped = scaled.min(self.max_delay);
        if kind == FailureKind::RateLimited {
            capped.max(self.rate_limit_floor.min(self.max_delay))
        } else {
            capped
        }
    }

    /// Should the caller retry after `attempt` failures (0-based count)?
    pub fn should_retry(&self, failures: u32, kind: FailureKind) -> bool {
        kind.retryable() && failures < self.max_attempts.saturating_sub(1)
    }

    /// The next state after a failure, or `None` when the caller must give up.
    pub fn next(&self, state: RetryState, kind: FailureKind) -> Option<RetryState> {
        if !self.should_retry(state.failures, kind) {
            return None;
        }
        Some(RetryState {
            failures: state.failures + 1,
            next_delay: self.delay(state.failures, kind),
        })
    }
}

/// Upper bound for a provider-supplied `Retry-After`, so a misconfigured
/// gateway cannot stall a turn for minutes (same ceiling as smart-ark).
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(30);

/// Relative jitter half-width (±25%), matching smart-ark's thundering-herd
/// desynchronisation range.
const JITTER_FRACTION: u64 = 25;

/// Parse a `Retry-After` response header (RFC 9110 §10.2.3).
///
/// Accepts delay-seconds or an HTTP-date; `None` when absent or unparseable.
/// The result is capped at [`RETRY_AFTER_CAP`].
pub fn parse_retry_after(header: Option<&str>, now: std::time::SystemTime) -> Option<Duration> {
    let header = header?.trim();
    if header.is_empty() {
        return None;
    }
    let parsed = match header.parse::<u64>() {
        // A negative value is unparseable as u64 and reads as "retry now".
        Ok(seconds) => Duration::from_secs(seconds),
        Err(_) => {
            let target = chrono_http_date(header)?;
            target.duration_since(now).ok()?
        }
    };
    Some(parsed.min(RETRY_AFTER_CAP))
}

/// Parse an HTTP-date `Retry-After` value. `parse_from_rfc2822` only accepts
/// the IMF-fixdate form (`Sun, 06 Nov 1994 08:49:37 GMT`); the two legacy
/// RFC 850 / asctime spellings fail to parse and the caller falls back to
/// exponential backoff, which is safe (merely provider-unadvised).
fn chrono_http_date(header: &str) -> Option<std::time::SystemTime> {
    use chrono::{DateTime, Utc};
    DateTime::parse_from_rfc2822(header)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).into())
}

/// Apply ±`JITTER_FRACTION`% jitter to a computed delay.
///
/// Deterministic in `(delay, salt)`: tests assert exact bounds, and production
/// salts with the current time so concurrent turns desynchronise without a
/// random-number dependency. Salt variation only shifts within the band, so
/// correctness never depends on the entropy source.
pub fn jittered(delay: Duration, salt: u64) -> Duration {
    let millis = delay.as_millis() as u64;
    if millis == 0 {
        return delay;
    }
    // SplitMix64-style finalizer variant: same no-dependency spirit, but the
    // constants and rotations differ from the standard SplitMix64 mix.
    let mut z = salt.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(30);
    z ^= z >> 27;
    z = z.wrapping_mul(0xBF58_476D_C166_45C9).rotate_left(23);
    let band = millis * JITTER_FRACTION / 100;
    let offset = (z % (2 * band + 1)) as i64 - band as i64;
    Duration::from_millis((millis as i64 + offset).max(0) as u64)
}

/// Attempt bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryState {
    /// Failures observed so far.
    pub failures: u32,
    /// Delay to wait before the next attempt.
    pub next_delay: Duration,
}

impl Default for RetryState {
    fn default() -> Self {
        Self {
            failures: 0,
            next_delay: Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 4,
            base: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            rate_limit_floor: Duration::from_millis(300),
        }
    }

    #[test]
    fn status_classification_is_total() {
        assert_eq!(FailureKind::from_status(500), FailureKind::Transient);
        assert_eq!(FailureKind::from_status(503), FailureKind::Transient);
        assert_eq!(FailureKind::from_status(429), FailureKind::RateLimited);
        assert_eq!(FailureKind::from_status(404), FailureKind::Permanent);
        assert_eq!(FailureKind::from_status(401), FailureKind::Permanent);
        assert_eq!(FailureKind::from_status(400), FailureKind::Permanent);
        assert_eq!(FailureKind::from_status(408), FailureKind::Transient);
    }

    #[test]
    fn permanent_failures_are_never_retried() {
        assert!(!FailureKind::Permanent.retryable());
        assert!(!policy().should_retry(0, FailureKind::Permanent));
        assert_eq!(
            policy().next(RetryState::default(), FailureKind::Permanent),
            None
        );
    }

    #[test]
    fn backoff_grows_exponentially() {
        let policy = policy();
        assert_eq!(
            policy.delay(0, FailureKind::Transient),
            Duration::from_millis(100)
        );
        assert_eq!(
            policy.delay(1, FailureKind::Transient),
            Duration::from_millis(200)
        );
        assert_eq!(
            policy.delay(2, FailureKind::Transient),
            Duration::from_millis(400)
        );
    }

    #[test]
    fn backoff_is_capped() {
        assert_eq!(
            policy().delay(9, FailureKind::Transient),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn rate_limits_get_the_floor() {
        // 100ms exponential is below the 300ms floor.
        assert_eq!(
            policy().delay(0, FailureKind::RateLimited),
            Duration::from_millis(300)
        );
        assert_eq!(
            policy().delay(5, FailureKind::RateLimited),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn attempt_budget_is_respected() {
        let policy = policy();
        // max_attempts = 4 means up to 3 retries.
        assert!(policy.should_retry(0, FailureKind::Transient));
        assert!(policy.should_retry(1, FailureKind::Transient));
        assert!(policy.should_retry(2, FailureKind::Transient));
        assert!(!policy.should_retry(3, FailureKind::Transient));
    }

    #[test]
    fn state_advances_until_it_gives_up() {
        let policy = policy();
        let mut state = RetryState::default();
        let mut steps = 0;
        while let Some(next) = policy.next(state, FailureKind::Transient) {
            state = next;
            steps += 1;
        }
        assert_eq!(steps, 3);
        assert_eq!(state.failures, 3);
    }

    #[test]
    fn no_retry_policy_gives_up_immediately() {
        assert_eq!(
            RetryPolicy::none().next(RetryState::default(), FailureKind::Transient),
            None
        );
    }

    #[test]
    fn default_policy_matches_smart_ark_budget() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_attempts, 4);
        assert_eq!(policy.base, Duration::from_millis(500));
        assert_eq!(policy.max_delay, Duration::from_secs(30));
    }

    #[test]
    fn retry_after_seconds_are_parsed_and_capped() {
        let now = std::time::SystemTime::now();
        assert_eq!(
            parse_retry_after(Some("2"), now),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            parse_retry_after(Some("3600"), now),
            Some(RETRY_AFTER_CAP),
            "a provider asking for an hour must be capped"
        );
        assert_eq!(parse_retry_after(Some("0"), now), Some(Duration::ZERO));
        assert_eq!(parse_retry_after(None, now), None);
        assert_eq!(parse_retry_after(Some(""), now), None);
        assert_eq!(parse_retry_after(Some("soon"), now), None);
        assert_eq!(parse_retry_after(Some("-5"), now), None);
    }

    #[test]
    fn retry_after_http_date_is_parsed() {
        use chrono::Utc;
        let now = Utc::now().into();
        let future = Utc::now() + chrono::Duration::seconds(10);
        let header = future.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let parsed = parse_retry_after(Some(&header), now).expect("date parses");
        assert!(parsed <= Duration::from_secs(10));
        assert!(parsed >= Duration::from_secs(8), "≈10s minus parse delay");
        let past = (Utc::now() - chrono::Duration::seconds(10)).to_rfc2822();
        assert_eq!(
            parse_retry_after(Some(&past), now),
            None,
            "a past date is not a wait"
        );
    }

    #[test]
    fn jitter_stays_within_the_quarter_band() {
        let delay = Duration::from_millis(2000);
        for salt in 0..200u64 {
            let jittered = jittered(delay, salt);
            assert!(jittered >= Duration::from_millis(1500));
            assert!(jittered <= Duration::from_millis(2500));
        }
        // Both band edges are reachable across salts: not a constant offset.
        let values: Vec<u64> = (0..500u64)
            .map(|s| jittered(delay, s).as_millis() as u64)
            .collect();
        assert!(values.iter().any(|&v| v < 2000));
        assert!(values.iter().any(|&v| v > 2000));
        assert!(!values.iter().all(|&v| v == values[0]));
    }

    #[test]
    fn zero_delay_is_not_jittered() {
        assert_eq!(jittered(Duration::ZERO, 42), Duration::ZERO);
    }
}
