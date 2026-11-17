//! The explicit turn state (design §7.1).
//!
//! The transition table is data, not scattered `if`s: [`TurnState::can_enter`]
//! is the single place that says which moves are legal, so an invalid jump is a
//! testable error instead of a mystery runtime behaviour.

use serde::{Deserialize, Serialize};

/// Every observable state of one turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    /// Build the request prefix and repair history.
    Preparing,
    /// A model stream is in flight. At most one of these per turn.
    AwaitingModel,
    /// Tool calls are being executed.
    DispatchingTools,
    /// Blocked on an approval gate.
    AwaitingApproval,
    /// Blocked on a user-input gate.
    AwaitingUserInput,
    /// Context compaction in progress.
    Compacting,
    /// Terminal artifacts are being written (terminal event, item close).
    Finalizing,
    Completed,
    Failed,
    Aborted,
}

impl TurnState {
    /// True for the three states a turn can never leave.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TurnState::Completed | TurnState::Failed | TurnState::Aborted
        )
    }

    /// Stable wire/log name. Kept identical to the serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            TurnState::Preparing => "preparing",
            TurnState::AwaitingModel => "awaiting_model",
            TurnState::DispatchingTools => "dispatching_tools",
            TurnState::AwaitingApproval => "awaiting_approval",
            TurnState::AwaitingUserInput => "awaiting_user_input",
            TurnState::Compacting => "compacting",
            TurnState::Finalizing => "finalizing",
            TurnState::Completed => "completed",
            TurnState::Failed => "failed",
            TurnState::Aborted => "aborted",
        }
    }

    /// Legal transitions. `Finalizing` may only be reached from a working state,
    /// and every terminal state must pass through `Finalizing` so the terminal
    /// event is written before the turn is declared over.
    pub fn can_enter(self, next: TurnState) -> bool {
        use TurnState::*;
        if self.is_terminal() {
            // A finished turn is immutable; only a retry creates a new turn.
            return false;
        }
        match (self, next) {
            // A compaction that did not shrink the prefix enough may run again
            // while budget remains; `note_compaction` bounds the retries.
            // `AwaitingUserInput` belongs here because that state may hand
            // straight to a model round (`after_user_input`), so it needs the
            // same "compact first" option every other model-calling state has.
            (_, Compacting) => matches!(
                self,
                Preparing | DispatchingTools | AwaitingModel | AwaitingUserInput | Compacting
            ),
            (Preparing, AwaitingModel) => true,
            // Any working state must be able to write a terminal event: an
            // I/O error during `prepare()` otherwise leaves a turn with a
            // `turn_started` and no terminal (AG-01).
            (Preparing, Finalizing) => true,
            (AwaitingModel, DispatchingTools) => true,
            (AwaitingModel, AwaitingApproval) => true,
            (AwaitingModel, AwaitingUserInput) => true,
            (AwaitingModel, Finalizing) => true,
            (DispatchingTools, AwaitingModel) => true,
            (DispatchingTools, AwaitingApproval) => true,
            (DispatchingTools, AwaitingUserInput) => true,
            (DispatchingTools, Finalizing) => true,
            (AwaitingApproval, DispatchingTools) => true,
            (AwaitingApproval, AwaitingApproval) => true,
            (AwaitingApproval, Finalizing) => true,
            (AwaitingUserInput, DispatchingTools) => true,
            (AwaitingUserInput, AwaitingModel) => true,
            (AwaitingUserInput, AwaitingUserInput) => true,
            // Asks are answered before approvals within one round, so the ask
            // chain hands over to the approval chain (A2).
            (AwaitingUserInput, AwaitingApproval) => true,
            (AwaitingUserInput, Finalizing) => true,
            (Compacting, AwaitingModel) => true,
            (Compacting, Finalizing) => true,
            (Finalizing, Completed) => true,
            (Finalizing, Failed) => true,
            (Finalizing, Aborted) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_is_walkable() {
        let path = [
            TurnState::Preparing,
            TurnState::AwaitingModel,
            TurnState::DispatchingTools,
            TurnState::AwaitingModel,
            TurnState::Finalizing,
            TurnState::Completed,
        ];
        for pair in path.windows(2) {
            assert!(pair[0].can_enter(pair[1]), "{:?} -> {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn skipped_finalizing_is_rejected() {
        // Design §7.1: the terminal event must exist before Completed.
        assert!(!TurnState::AwaitingModel.can_enter(TurnState::Completed));
        assert!(!TurnState::Preparing.can_enter(TurnState::Completed));
    }

    #[test]
    fn a_failed_prepare_can_still_reach_finalizing() {
        // A write failure during `prepare()` must still converge to a terminal
        // event instead of leaving an orphaned `turn_started` (AG-01).
        assert!(TurnState::Preparing.can_enter(TurnState::Finalizing));
    }

    #[test]
    fn terminal_states_are_frozen() {
        for state in [TurnState::Completed, TurnState::Failed, TurnState::Aborted] {
            assert!(state.is_terminal());
            assert!(!state.can_enter(TurnState::Preparing));
        }
    }

    #[test]
    fn cannot_await_model_from_awaiting_model() {
        // Guards the "no two concurrent model calls per turn" invariant.
        assert!(!TurnState::AwaitingModel.can_enter(TurnState::AwaitingModel));
    }

    #[test]
    fn gate_states_reenter_their_own_state() {
        // A mixed model round queues several gates; each resolved gate asks the
        // next one while the turn stays blocked on the same gate kind. Without
        // these self transitions the second ask of a chain is an illegal move.
        assert!(TurnState::AwaitingApproval.can_enter(TurnState::AwaitingApproval));
        assert!(TurnState::AwaitingUserInput.can_enter(TurnState::AwaitingUserInput));
        // The ask chain of one round ends in its approval chain (A2).
        assert!(TurnState::AwaitingUserInput.can_enter(TurnState::AwaitingApproval));
    }

    #[test]
    fn compaction_can_retry_until_budget_is_spent() {
        assert!(TurnState::Compacting.can_enter(TurnState::Compacting));
        assert!(TurnState::Preparing.can_enter(TurnState::Compacting));
        assert!(!TurnState::Finalizing.can_enter(TurnState::Compacting));
        assert!(!TurnState::AwaitingApproval.can_enter(TurnState::Compacting));
    }

    #[test]
    fn every_state_that_can_call_the_model_can_compact_first() {
        // `compact_or_call` is the one round gate; a state that may reach the
        // model directly must also be allowed to fold first, or the new gate
        // turns into an illegal transition at runtime.
        for state in [
            TurnState::Preparing,
            TurnState::AwaitingModel,
            TurnState::DispatchingTools,
            TurnState::AwaitingUserInput,
        ] {
            assert!(
                state.can_enter(TurnState::Compacting),
                "{:?} must be able to compact before a model round",
                state
            );
        }
    }

    #[test]
    fn wire_names_are_snake_case() {
        assert_eq!(TurnState::AwaitingModel.as_str(), "awaiting_model");
        let json = serde_json::to_string(&TurnState::AwaitingApproval).unwrap();
        assert_eq!(json, "\"awaiting_approval\"");
    }
}
