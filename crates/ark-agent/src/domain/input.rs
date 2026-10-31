//! What the pure step reacts to.
//!
//! [`TurnInput`] carries a *snapshot* of the counters plus the limits, so
//! [`crate::service::step`] can decide budgets without touching shared state
//! and without a second source of truth. Approval verdicts arrive as data for
//! the same reason: the step never consults a policy object.

use ark_common::Usage;

use crate::domain::action::SyntheticResult;
use crate::domain::limits::TurnLimits;
use crate::domain::tool::{ToolCallRequest, ToolExecution};

/// Cumulative counters for the turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub rounds: u32,
    pub tokens_used: u64,
    pub tool_calls: u32,
    pub compactions: u32,
    /// Token estimate of the request prefix the next model call would send.
    pub prompt_estimate: u64,
}

impl Progress {
    /// Counters as they would be after folding one usage sample in.
    pub fn with_usage(&self, usage: &Usage) -> Self {
        let mut next = *self;
        next.tokens_used = next.tokens_used.saturating_add(usage.total_tokens);
        next
    }
}

/// A tool call the sandbox wants a human to approve before it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalAsk {
    pub call: ToolCallRequest,
    pub summary: String,
}

impl ApprovalAsk {
    pub fn new(call: ToolCallRequest, summary: impl Into<String>) -> Self {
        Self {
            call,
            summary: summary.into(),
        }
    }
}

/// A `request_user_input` call intercepted before dispatch: the prompt was
/// already extracted from the arguments, so the step can emit `Action::AskUser`
/// without re-parsing JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct UserInputAsk {
    pub call: ToolCallRequest,
    pub prompt: String,
}

impl UserInputAsk {
    pub fn new(call: ToolCallRequest, prompt: impl Into<String>) -> Self {
        Self {
            call,
            prompt: prompt.into(),
        }
    }
}

/// The event that moves the state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    /// A new turn: the user text is already appended to the history.
    Start,
    /// The model stream finished.
    ModelSettled {
        text: String,
        tool_calls: Vec<ToolCallRequest>,
        /// How many times each signature already ran this turn, aligned with
        /// `tool_calls`. Computed by the machine, consumed by the policy.
        repeats: Vec<u32>,
        usage: Usage,
    },
    /// The model stream failed. Retries happen inside the adapter (§7.2).
    ModelFailed { message: String },
    /// Tools finished (real executions plus refused calls).
    ToolsSettled { executions: Vec<ToolExecution> },
    /// One gate was resolved. The rest of the gate chain travels with it so the
    /// step stays a pure function of its input.
    GateResolved {
        call: ToolCallRequest,
        approved: bool,
        reason: String,
        /// The next call still waiting for a human, if any.
        next_ask: Option<ApprovalAsk>,
        /// Calls that may run once the chain is resolved.
        pending_dispatch: Vec<ToolCallRequest>,
        /// Synthetic results accumulated from earlier denials in this chain.
        synthetic_so_far: Vec<SyntheticResult>,
        /// Repeat counters aligned with `pending_dispatch`.
        repeats: Vec<u32>,
    },
    /// A user-input gate was answered; the answer becomes the tool result.
    UserInputResolved {
        call: ToolCallRequest,
        answer: String,
    },
    /// Compaction finished.
    CompactionSettled { removed_items: u32 },
    /// Interrupt requested from outside the loop.
    Interrupt { reason: String },
}

/// One step's complete input.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnInput {
    pub limits: TurnLimits,
    pub progress: Progress,
    pub kind: InputKind,
    /// Calls that must pass the approval gate before they may run. Empty means
    /// "nothing to ask": the step will dispatch directly.
    pub approvals: Vec<ApprovalAsk>,
    /// `request_user_input` calls intercepted before dispatch.
    pub user_inputs: Vec<UserInputAsk>,
}

impl TurnInput {
    pub fn new(limits: TurnLimits, progress: Progress, kind: InputKind) -> Self {
        Self {
            limits,
            progress,
            kind,
            approvals: Vec::new(),
            user_inputs: Vec::new(),
        }
    }

    pub fn with_approvals(mut self, approvals: Vec<ApprovalAsk>) -> Self {
        self.approvals = approvals;
        self
    }

    pub fn with_user_inputs(mut self, user_inputs: Vec<UserInputAsk>) -> Self {
        self.user_inputs = user_inputs;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_is_projected_without_mutating_the_snapshot() {
        let progress = Progress {
            tokens_used: 100,
            ..Progress::default()
        };
        let projected = progress.with_usage(&Usage::new(30, 20));
        assert_eq!(projected.tokens_used, 150);
        assert_eq!(progress.tokens_used, 100, "snapshot stays immutable");
    }

    #[test]
    fn projection_saturates_instead_of_overflowing() {
        let progress = Progress {
            tokens_used: u64::MAX - 1,
            ..Progress::default()
        };
        assert_eq!(
            progress.with_usage(&Usage::new(10, 10)).tokens_used,
            u64::MAX
        );
    }

    #[test]
    fn approvals_default_to_empty() {
        let input = TurnInput::new(TurnLimits::tiny(), Progress::default(), InputKind::Start);
        assert!(input.approvals.is_empty());
    }

    #[test]
    fn approvals_can_be_attached() {
        let call = ToolCallRequest::new("c1", "bash", serde_json::json!({}));
        let input = TurnInput::new(TurnLimits::tiny(), Progress::default(), InputKind::Start)
            .with_approvals(vec![ApprovalAsk::new(call, "run it")]);
        assert_eq!(input.approvals.len(), 1);
        assert_eq!(input.approvals[0].summary, "run it");
    }

    #[test]
    fn user_inputs_can_be_attached() {
        let call = ToolCallRequest::new("c1", "request_user_input", serde_json::json!({}));
        let input = TurnInput::new(TurnLimits::tiny(), Progress::default(), InputKind::Start)
            .with_user_inputs(vec![UserInputAsk::new(call, "which branch?")]);
        assert_eq!(input.user_inputs.len(), 1);
        assert_eq!(input.user_inputs[0].prompt, "which branch?");
    }
}
