//! Turn budget accounting (design §7.1 `policy/`).
//!
//! One place decides "may this turn continue". The reactor never re-checks a
//! limit on its own, so the reported reason is always the same one the decision
//! used.

use crate::domain::input::Progress;
use crate::domain::limits::TurnLimits;

/// Why a budget refused to continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetVerdict {
    /// There is headroom for another round.
    Allow,
    /// `max_rounds` reached.
    RoundsExhausted,
    /// `max_tokens` reached.
    TokensExhausted,
    /// `max_tool_calls` reached.
    ToolCallsExhausted,
    /// `max_compactions` reached.
    CompactionsExhausted,
    /// The next prompt would exceed `context_limit_tokens` and compaction has
    /// already been spent.
    ContextExhausted,
}

impl BudgetVerdict {
    pub fn allowed(self) -> bool {
        matches!(self, BudgetVerdict::Allow)
    }

    /// Message for the terminal `turn_failed` event. `None` when allowed.
    pub fn message(self) -> Option<&'static str> {
        match self {
            BudgetVerdict::Allow => None,
            BudgetVerdict::RoundsExhausted => Some("turn budget: max model rounds reached"),
            BudgetVerdict::TokensExhausted => Some("turn budget: token budget reached"),
            BudgetVerdict::ToolCallsExhausted => Some("turn budget: max tool calls reached"),
            BudgetVerdict::CompactionsExhausted => Some("turn budget: compaction limit reached"),
            BudgetVerdict::ContextExhausted => {
                Some("turn budget: context limit reached without compaction headroom")
            }
        }
    }
}

/// Stateless budget calculator.
#[derive(Debug, Clone, Copy)]
pub struct TurnBudget {
    limits: TurnLimits,
}

impl TurnBudget {
    pub fn new(limits: TurnLimits) -> Self {
        Self { limits }
    }

    pub fn limits(&self) -> TurnLimits {
        self.limits
    }

    /// May the turn start another model round?
    ///
    /// Round and token limits are checked first: they are unconditional. The
    /// context limit is only fatal when compaction has no headroom left,
    /// otherwise the caller compacts instead of failing.
    pub fn check(&self, progress: &Progress) -> BudgetVerdict {
        if progress.rounds >= self.limits.max_rounds {
            return BudgetVerdict::RoundsExhausted;
        }
        if progress.tool_calls >= self.limits.max_tool_calls {
            return BudgetVerdict::ToolCallsExhausted;
        }
        if progress.tokens_used >= self.limits.max_tokens {
            return BudgetVerdict::TokensExhausted;
        }
        if self.needs_compaction(progress.prompt_estimate) && !self.compaction_available(progress) {
            return BudgetVerdict::ContextExhausted;
        }
        BudgetVerdict::Allow
    }

    /// True when the estimated prompt no longer fits the context window.
    pub fn needs_compaction(&self, prompt_estimate: u64) -> bool {
        prompt_estimate > self.limits.context_limit_tokens
    }

    /// True when a compaction attempt is still within budget.
    pub fn compaction_available(&self, progress: &Progress) -> bool {
        progress.compactions < self.limits.max_compactions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> TurnBudget {
        TurnBudget::new(TurnLimits {
            max_rounds: 3,
            max_tokens: 100,
            max_tool_calls: 5,
            ..TurnLimits::tiny()
        })
    }

    #[test]
    fn fresh_turn_is_allowed() {
        assert_eq!(budget().check(&Progress::default()), BudgetVerdict::Allow);
        assert!(budget().check(&Progress::default()).message().is_none());
    }

    #[test]
    fn round_limit_is_enforced() {
        let progress = Progress {
            rounds: 3,
            ..Progress::default()
        };
        assert_eq!(budget().check(&progress), BudgetVerdict::RoundsExhausted);
        assert!(budget()
            .check(&progress)
            .message()
            .unwrap()
            .contains("rounds"));
    }

    #[test]
    fn token_limit_is_enforced() {
        let progress = Progress {
            tokens_used: 100,
            ..Progress::default()
        };
        assert_eq!(budget().check(&progress), BudgetVerdict::TokensExhausted);
    }

    #[test]
    fn tool_call_limit_is_enforced() {
        let progress = Progress {
            tool_calls: 5,
            ..Progress::default()
        };
        assert_eq!(budget().check(&progress), BudgetVerdict::ToolCallsExhausted);
    }

    #[test]
    fn large_prompt_compacts_before_failing() {
        let progress = Progress {
            prompt_estimate: 9_999,
            compactions: 0,
            ..Progress::default()
        };
        assert!(budget().needs_compaction(progress.prompt_estimate));
        assert!(budget().compaction_available(&progress));
        // Budget says "allowed" because compaction is still available.
        assert_eq!(budget().check(&progress), BudgetVerdict::Allow);
    }

    #[test]
    fn large_prompt_fails_when_compaction_is_spent() {
        let progress = Progress {
            prompt_estimate: 9_999,
            compactions: 1,
            ..Progress::default()
        };
        assert_eq!(budget().check(&progress), BudgetVerdict::ContextExhausted);
    }

    #[test]
    fn small_prompt_never_asks_for_compaction() {
        assert!(!budget().needs_compaction(10));
    }
}
