//! Turn budgets. One struct, consulted by both the pure step and the reactor so
//! a limit can never be enforced in two places with two different meanings.

use serde::{Deserialize, Serialize};

/// Hard limits for a single turn.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TurnLimits {
    /// Maximum model round-trips in one turn.
    pub max_rounds: u32,
    /// Maximum cumulative tokens (prompt + completion) in one turn.
    pub max_tokens: u64,
    /// Maximum number of tool calls in one turn.
    pub max_tool_calls: u32,
    /// A call whose signature repeats at least this many times is suppressed.
    pub max_identical_calls: u32,
    /// Maximum compaction attempts per turn (prevents a compaction loop).
    pub max_compactions: u32,
    /// Prompt estimate above which compaction is triggered.
    pub context_limit_tokens: u64,
    /// Messages kept verbatim at the tail when compacting.
    pub keep_recent_messages: usize,
}

impl Default for TurnLimits {
    fn default() -> Self {
        Self {
            max_rounds: 24,
            max_tokens: 1_000_000,
            max_tool_calls: 200,
            max_identical_calls: 3,
            max_compactions: 2,
            context_limit_tokens: 96_000,
            keep_recent_messages: 12,
        }
    }
}

impl TurnLimits {
    /// Small, predictable limits for tests that must not depend on defaults.
    pub fn tiny() -> Self {
        Self {
            max_rounds: 4,
            max_tokens: 10_000,
            max_tool_calls: 8,
            max_identical_calls: 2,
            max_compactions: 1,
            context_limit_tokens: 2_000,
            keep_recent_messages: 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_ordered_sensibly() {
        let limits = TurnLimits::default();
        assert!(limits.max_rounds > 0);
        assert!(limits.max_tool_calls >= limits.max_rounds);
        assert!(limits.keep_recent_messages > 0);
    }

    #[test]
    fn tiny_limits_fit_tests() {
        let limits = TurnLimits::tiny();
        assert_eq!(limits.max_identical_calls, 2);
        assert!(limits.context_limit_tokens < TurnLimits::default().context_limit_tokens);
    }
}
