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
        Self {
            max_attempts: 3,
            base: Duration::from_millis(400),
            max_delay: Duration::from_secs(8),
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
    /// failure). `attempt * base`, capped, floored for rate limiting.
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
}
