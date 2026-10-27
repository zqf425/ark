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
            (_, Compacting) => matches!(self, Preparing | DispatchingTools | AwaitingModel),
            (Preparing, AwaitingModel) => true,
            (AwaitingModel, DispatchingTools) => true,
            (AwaitingModel, AwaitingApproval) => true,
            (AwaitingModel, AwaitingUserInput) => true,
            (AwaitingModel, Finalizing) => true,
            (DispatchingTools, AwaitingModel) => true,
            (DispatchingTools, AwaitingApproval) => true,
            (DispatchingTools, AwaitingUserInput) => true,
            (DispatchingTools, Finalizing) => true,
            (AwaitingApproval, DispatchingTools) => true,
            (AwaitingApproval, Finalizing) => true,
            (AwaitingUserInput, DispatchingTools) => true,
            (AwaitingUserInput, AwaitingModel) => true,
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
    fn wire_names_are_snake_case() {
        assert_eq!(TurnState::AwaitingModel.as_str(), "awaiting_model");
        let json = serde_json::to_string(&TurnState::AwaitingApproval).unwrap();
        assert_eq!(json, "\"awaiting_approval\"");
    }
}
