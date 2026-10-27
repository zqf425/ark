//! Actions: the *only* output of the pure step (design §7.1).
//!
//! An [`Action`] is a description of one boundary effect, never the effect
//! itself. The reactor translates exactly one action into calls on
//! `ModelClient` / `Tool` / the journal, which keeps the decision logic free of
//! I/O and makes every transition assertable in a unit test.

use serde::{Deserialize, Serialize};

use crate::domain::state::TurnState;
use crate::domain::tool::ToolCallRequest;

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Failed { message: String },
    Aborted { reason: String },
}

impl TurnOutcome {
    pub fn failed(message: impl Into<String>) -> Self {
        TurnOutcome::Failed {
            message: message.into(),
        }
    }

    pub fn aborted(reason: impl Into<String>) -> Self {
        TurnOutcome::Aborted {
            reason: reason.into(),
        }
    }

    /// Terminal state this outcome lands on.
    pub fn terminal_state(&self) -> TurnState {
        match self {
            TurnOutcome::Completed => TurnState::Completed,
            TurnOutcome::Failed { .. } => TurnState::Failed,
            TurnOutcome::Aborted { .. } => TurnState::Aborted,
        }
    }
}

/// A tool call that will *not* run, together with the tool result that must be
/// written anyway. Dropping the result would break the tool_call/tool_result
/// pairing the provider requires, so the reactor always emits it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyntheticResult {
    pub call_id: String,
    pub tool: String,
    pub ok: bool,
    pub content: String,
    /// Set when the storm breaker refused the call, so the journal can record
    /// how many times the signature was seen. `None` for gate denials.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storm_count: Option<u32>,
}

impl SyntheticResult {
    pub fn new(call: &ToolCallRequest, ok: bool, content: impl Into<String>) -> Self {
        Self {
            call_id: call.call_id.clone(),
            tool: call.tool.clone(),
            ok,
            content: content.into(),
            storm_count: None,
        }
    }

    /// Mark the refusal as a storm suppression with the observed repeat count.
    pub fn with_storm_count(mut self, count: u32) -> Self {
        self.storm_count = Some(count);
        self
    }
}

/// The tool-storm breaker's finding, surfaced as an event and a synthetic result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StormNote {
    pub tool: String,
    pub signature: String,
    pub count: u32,
}

/// Everything the reactor needs to run one tool phase.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DispatchPlan {
    /// Batches run one after another; the calls **inside** a batch are safe to
    /// run concurrently (decided by `policy::parallel`, never by the reactor).
    pub batches: Vec<Vec<ToolCallRequest>>,
    /// Calls that must be answered without executing the tool.
    pub synthetic_results: Vec<SyntheticResult>,
}

impl DispatchPlan {
    /// Total number of calls this plan resolves (executed + synthetic).
    pub fn total_calls(&self) -> usize {
        self.batches.iter().map(|batch| batch.len()).sum::<usize>() + self.synthetic_results.len()
    }

    pub fn is_empty(&self) -> bool {
        self.batches.is_empty() && self.synthetic_results.is_empty()
    }
}

/// One step's decision.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Start the next model round.
    CallModel { round: u32 },
    /// Run tools (see [`DispatchPlan`]).
    Dispatch(DispatchPlan),
    /// Block on an approval gate.
    RequestApproval {
        call: ToolCallRequest,
        summary: String,
    },
    /// Block on a user-input gate.
    AskUser {
        call: ToolCallRequest,
        prompt: String,
    },
    /// Compact the context prefix.
    Compact { keep_recent: usize },
    /// Write the terminal artifacts, then land on `outcome.terminal_state()`.
    Finish { outcome: TurnOutcome },
    /// Internally inconsistent `(state, input)` pair. Fails loudly instead of
    /// silently doing nothing, so a logic bug surfaces in the first test run.
    Invalid { reason: String },
}

impl Action {
    /// State the machine must move to in order to execute this action. `None`
    /// means "no transition" (invalid action).
    pub fn target_state(&self) -> Option<TurnState> {
        match self {
            Action::CallModel { .. } => Some(TurnState::AwaitingModel),
            Action::Dispatch(_) => Some(TurnState::DispatchingTools),
            Action::RequestApproval { .. } => Some(TurnState::AwaitingApproval),
            Action::AskUser { .. } => Some(TurnState::AwaitingUserInput),
            Action::Compact { .. } => Some(TurnState::Compacting),
            Action::Finish { .. } => Some(TurnState::Finalizing),
            Action::Invalid { .. } => None,
        }
    }

    pub fn is_invalid(&self) -> bool {
        matches!(self, Action::Invalid { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_maps_to_a_state_its_origin_allows() {
        let call = ToolCallRequest::new("c1", "read", serde_json::json!({"path": "a"}));
        let cases = [
            (TurnState::Preparing, Action::CallModel { round: 1 }),
            (TurnState::Preparing, Action::Compact { keep_recent: 4 }),
            (
                TurnState::AwaitingModel,
                Action::Dispatch(DispatchPlan::default()),
            ),
            (
                TurnState::AwaitingModel,
                Action::RequestApproval {
                    call: call.clone(),
                    summary: "s".into(),
                },
            ),
            (
                TurnState::AwaitingModel,
                Action::AskUser {
                    call: call.clone(),
                    prompt: "p".into(),
                },
            ),
            (
                TurnState::AwaitingModel,
                Action::Finish {
                    outcome: TurnOutcome::Completed,
                },
            ),
        ];
        for (from, action) in cases {
            let target = action
                .target_state()
                .expect("non-invalid action has a state");
            assert!(from.can_enter(target), "{:?} from {:?}", action, from);
        }
    }

    #[test]
    fn dispatch_is_never_legal_straight_from_preparing() {
        // The first model round must happen before any tool can be dispatched.
        let target = Action::Dispatch(DispatchPlan::default())
            .target_state()
            .unwrap();
        assert!(!TurnState::Preparing.can_enter(target));
    }

    #[test]
    fn finish_targets_finalizing_not_completed() {
        // Design §7.1: the terminal event is written in Finalizing.
        let action = Action::Finish {
            outcome: TurnOutcome::Completed,
        };
        assert_eq!(action.target_state(), Some(TurnState::Finalizing));
        assert_eq!(
            TurnOutcome::Completed.terminal_state(),
            TurnState::Completed
        );
    }

    #[test]
    fn invalid_has_no_state() {
        let action = Action::Invalid {
            reason: "boom".into(),
        };
        assert!(action.is_invalid());
        assert_eq!(action.target_state(), None);
    }

    #[test]
    fn plan_counts_synthetic_results_too() {
        let call = ToolCallRequest::new("c1", "read", serde_json::json!({}));
        let plan = DispatchPlan {
            batches: vec![vec![call.clone()], vec![call.clone()]],
            synthetic_results: vec![SyntheticResult::new(&call, false, "denied")],
        };
        assert_eq!(plan.total_calls(), 3);
        assert!(!plan.is_empty());
        assert!(DispatchPlan::default().is_empty());
    }

    #[test]
    fn outcome_terminal_states_are_distinct() {
        assert_eq!(TurnOutcome::failed("x").terminal_state(), TurnState::Failed);
        assert_eq!(
            TurnOutcome::aborted("x").terminal_state(),
            TurnState::Aborted
        );
    }
}
