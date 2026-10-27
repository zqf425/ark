//! Tool-storm breaker (design §7.1 `policy/`).
//!
//! A model that keeps re-issuing the same call is a failure mode, not progress.
//! The breaker refuses the *n*-th identical call and, critically, hands back a
//! synthetic tool result for it: dropping the call outright would leave an
//! unanswered `tool_call` and poison the history for every later round.

use crate::domain::tool::ToolCallRequest;

/// One refused call.
#[derive(Debug, Clone, PartialEq)]
pub struct SuppressedCall {
    pub call: ToolCallRequest,
    /// How many times this exact signature will have been seen, including now.
    pub count: u32,
}

/// Result of one storm check.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StormReport {
    pub allowed: Vec<ToolCallRequest>,
    pub suppressed: Vec<SuppressedCall>,
}

impl StormReport {
    pub fn is_suppressed(&self) -> bool {
        !self.suppressed.is_empty()
    }
}

/// Stateless breaker; the caller owns the repeat counters.
#[derive(Debug, Clone, Copy)]
pub struct StormBreaker {
    max_identical_calls: u32,
}

impl StormBreaker {
    pub fn new(max_identical_calls: u32) -> Self {
        Self {
            max_identical_calls: max_identical_calls.max(1),
        }
    }

    /// Split calls into "run these" and "refuse these".
    ///
    /// `repeats[i]` is how many times `calls[i]`'s signature already ran in this
    /// turn, so the call is refused once `repeats[i] + 1 > max_identical_calls`.
    /// A `repeats` slice shorter than `calls` is treated as zeros.
    pub fn check(&self, calls: &[ToolCallRequest], repeats: &[u32]) -> StormReport {
        let mut report = StormReport::default();
        for (idx, call) in calls.iter().enumerate() {
            let already = repeats.get(idx).copied().unwrap_or(0);
            let count = already.saturating_add(1);
            if count > self.max_identical_calls {
                report.suppressed.push(SuppressedCall {
                    call: call.clone(),
                    count,
                });
            } else {
                report.allowed.push(call.clone());
            }
        }
        report
    }

    pub fn limit(&self) -> u32 {
        self.max_identical_calls
    }
}

/// The result text written for a refused call. Kept here so the wording is
/// stable and testable rather than assembled at a call site.
pub fn suppression_message(tool: &str, count: u32) -> String {
    format!(
        "tool call suppressed by storm breaker: `{tool}` has already run {count} times in this turn; \
         stop repeating it and change approach or finish the turn"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, tool: &str, path: &str) -> ToolCallRequest {
        ToolCallRequest::new(id, tool, serde_json::json!({ "path": path }))
    }

    #[test]
    fn first_call_is_always_allowed() {
        let breaker = StormBreaker::new(3);
        let calls = vec![call("c1", "read", "a")];
        let report = breaker.check(&calls, &[0]);
        assert_eq!(report.allowed.len(), 1);
        assert!(!report.is_suppressed());
    }

    #[test]
    fn refusal_starts_after_the_limit() {
        let breaker = StormBreaker::new(2);
        let calls = vec![call("c3", "read", "a")];
        // Third occurrence: 2 repeats + this one = 3 > 2.
        let report = breaker.check(&calls, &[2]);
        assert!(report.allowed.is_empty());
        assert_eq!(report.suppressed[0].count, 3);
    }

    #[test]
    fn limit_boundary_is_inclusive() {
        let breaker = StormBreaker::new(2);
        let calls = vec![call("c2", "read", "a")];
        // Second occurrence: 1 repeat + this one = 2, not above the limit.
        assert!(breaker.check(&calls, &[1]).suppressed.is_empty());
    }

    #[test]
    fn missing_repeat_entries_default_to_zero() {
        let breaker = StormBreaker::new(1);
        let calls = vec![call("c1", "read", "a"), call("c2", "read", "b")];
        let report = breaker.check(&calls, &[]);
        assert_eq!(report.allowed.len(), 2);
    }

    #[test]
    fn zero_limit_is_clamped_to_one() {
        assert_eq!(StormBreaker::new(0).limit(), 1);
    }

    #[test]
    fn suppression_message_names_the_tool_and_count() {
        let message = suppression_message("bash", 4);
        assert!(message.contains("bash"));
        assert!(message.contains('4'));
    }
}
