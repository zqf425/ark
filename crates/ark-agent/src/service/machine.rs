//! The mutable half of the turn: state, counters and invariants.
//!
//! Design §7.1 requires two invariants to be *asserted*, not merely intended:
//!
//! 1. a turn never reaches `Completed` without a terminal event having been
//!    written ([`TurnMachine::complete`]);
//! 2. one turn never runs two model rounds at the same time
//!    ([`TurnMachine::enter`] rejects `AwaitingModel -> AwaitingModel`).
//!
//! Debug builds additionally assert both in [`TurnMachine::assert_invariants`],
//! so a regression fails in tests rather than in production.

use std::collections::BTreeMap;

use ark_common::Usage;

use crate::domain::action::{Action, TurnOutcome};
use crate::domain::input::Progress;
use crate::domain::limits::TurnLimits;
use crate::domain::state::TurnState;
use crate::domain::tool::ToolCallRequest;

/// Invariant violations. Every variant is a programming error, never user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineError {
    /// The transition table forbids this move.
    IllegalTransition { from: TurnState, to: TurnState },
    /// A second model round started while one was in flight.
    ConcurrentModelRound,
    /// `Completed` was reached without a terminal event.
    MissingTerminalEvent,
    /// The turn is already over.
    AlreadyTerminal { state: TurnState },
}

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MachineError::IllegalTransition { from, to } => {
                write!(
                    f,
                    "illegal turn transition {} -> {}",
                    from.as_str(),
                    to.as_str()
                )
            }
            MachineError::ConcurrentModelRound => {
                write!(f, "a second model round started while one was in flight")
            }
            MachineError::MissingTerminalEvent => {
                write!(f, "turn reached completed without a terminal event")
            }
            MachineError::AlreadyTerminal { state } => {
                write!(f, "turn is already terminal ({})", state.as_str())
            }
        }
    }
}

impl std::error::Error for MachineError {}

/// Owns the turn's mutable state.
#[derive(Debug, Clone)]
pub struct TurnMachine {
    state: TurnState,
    limits: TurnLimits,
    progress: Progress,
    repeats: BTreeMap<String, u32>,
    terminal_written: bool,
    model_in_flight: bool,
}

impl TurnMachine {
    pub fn new(limits: TurnLimits) -> Self {
        Self {
            state: TurnState::Preparing,
            limits,
            progress: Progress::default(),
            repeats: BTreeMap::new(),
            terminal_written: false,
            model_in_flight: false,
        }
    }

    pub fn state(&self) -> TurnState {
        self.state
    }

    pub fn limits(&self) -> TurnLimits {
        self.limits
    }

    pub fn progress(&self) -> Progress {
        self.progress
    }

    pub fn terminal_written(&self) -> bool {
        self.terminal_written
    }

    /// Update the prompt estimate the budget check reads.
    pub fn set_prompt_estimate(&mut self, tokens: u64) {
        self.progress.prompt_estimate = tokens;
    }

    /// Fold one usage sample into the turn totals.
    pub fn note_usage(&mut self, usage: &Usage) {
        self.progress = self.progress.with_usage(usage);
    }

    /// Count a finished tool phase.
    pub fn note_tool_calls(&mut self, count: u32) {
        self.progress.tool_calls = self.progress.tool_calls.saturating_add(count);
    }

    /// Count one compaction attempt.
    pub fn note_compaction(&mut self, removed_items: u32) {
        self.progress.compactions = self.progress.compactions.saturating_add(1);
        let _ = removed_items;
    }

    /// Record that these calls ran, so the storm breaker can count repeats.
    pub fn record_calls(&mut self, calls: &[ToolCallRequest]) {
        for call in calls {
            let entry = self.repeats.entry(call.signature()).or_insert(0);
            *entry = entry.saturating_add(1);
        }
    }

    /// Repeat counters aligned with `calls`, as the step expects them.
    pub fn repeats_for(&self, calls: &[ToolCallRequest]) -> Vec<u32> {
        calls
            .iter()
            .map(|call| self.repeats.get(&call.signature()).copied().unwrap_or(0))
            .collect()
    }

    /// Apply an action's target state, enforcing the invariants.
    pub fn enter(&mut self, action: &Action) -> Result<TurnState, MachineError> {
        let Some(target) = action.target_state() else {
            return Err(MachineError::IllegalTransition {
                from: self.state,
                to: self.state,
            });
        };
        if self.state.is_terminal() {
            return Err(MachineError::AlreadyTerminal { state: self.state });
        }
        if target == TurnState::AwaitingModel {
            if self.model_in_flight {
                return Err(MachineError::ConcurrentModelRound);
            }
            self.progress.rounds = self.progress.rounds.saturating_add(1);
        }
        if !self.state.can_enter(target) {
            return Err(MachineError::IllegalTransition {
                from: self.state,
                to: target,
            });
        }
        self.model_in_flight = target == TurnState::AwaitingModel;
        self.state = target;
        self.assert_invariants();
        Ok(target)
    }

    /// Record the terminal event. Must happen while still in `Finalizing`.
    pub fn mark_terminal(&mut self) -> Result<(), MachineError> {
        if self.state != TurnState::Finalizing {
            return Err(MachineError::IllegalTransition {
                from: self.state,
                to: TurnState::Finalizing,
            });
        }
        self.terminal_written = true;
        Ok(())
    }

    /// Land on `Completed` — refuses when no terminal event was written.
    pub fn complete(&mut self) -> Result<(), MachineError> {
        if !self.terminal_written {
            return Err(MachineError::MissingTerminalEvent);
        }
        self.land(TurnState::Completed)
    }

    /// Land on `Failed` (terminal event required for symmetry).
    pub fn fail(&mut self) -> Result<(), MachineError> {
        if !self.terminal_written {
            return Err(MachineError::MissingTerminalEvent);
        }
        self.land(TurnState::Failed)
    }

    /// Land on `Aborted`.
    pub fn abort(&mut self) -> Result<(), MachineError> {
        self.land(TurnState::Aborted)
    }

    fn land(&mut self, target: TurnState) -> Result<(), MachineError> {
        if self.state.is_terminal() {
            return Err(MachineError::AlreadyTerminal { state: self.state });
        }
        // Terminal states are reached through Finalizing only; an abort may cut
        // in directly because interruption is allowed from any working state.
        let allowed = if target == TurnState::Aborted {
            true
        } else {
            self.state == TurnState::Finalizing
        };
        if !allowed {
            return Err(MachineError::IllegalTransition {
                from: self.state,
                to: target,
            });
        }
        self.model_in_flight = false;
        self.state = target;
        self.assert_invariants();
        Ok(())
    }

    /// Debug-time invariant assertions (cheap, so also run in release builds'
    /// tests via `debug_assert!`).
    pub fn assert_invariants(&self) {
        debug_assert!(
            !(self.state == TurnState::AwaitingModel && self.model_in_flight == false),
            "awaiting_model must have a model round in flight"
        );
        debug_assert!(
            !(self.state.is_terminal() && self.model_in_flight),
            "a finished turn cannot have a model round in flight"
        );
        debug_assert!(
            !(self.state == TurnState::Completed && !self.terminal_written),
            "completed requires a terminal event"
        );
    }

    /// Terminal state implied by an outcome.
    pub fn settle(&mut self, outcome: &TurnOutcome) -> Result<(), MachineError> {
        match outcome.terminal_state() {
            TurnState::Completed => self.complete(),
            TurnState::Failed => self.fail(),
            _ => self.abort(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::action::TurnOutcome;

    fn machine() -> TurnMachine {
        TurnMachine::new(TurnLimits::tiny())
    }

    fn start_action() -> Action {
        Action::CallModel { round: 1 }
    }

    #[test]
    fn starts_in_preparing() {
        assert_eq!(machine().state(), TurnState::Preparing);
        assert_eq!(machine().progress().rounds, 0);
    }

    #[test]
    fn entering_awaiting_model_counts_a_round() {
        let mut machine = machine();
        machine.enter(&start_action()).unwrap();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
        assert_eq!(machine.progress().rounds, 1);
    }

    #[test]
    fn second_model_round_in_flight_is_rejected() {
        let mut machine = machine();
        machine.enter(&start_action()).unwrap();
        // Force the state back without clearing the in-flight flag: exactly the
        // regression this invariant exists for.
        machine.model_in_flight = true;
        machine.state = TurnState::Preparing;
        let error = machine.enter(&start_action()).unwrap_err();
        assert_eq!(error, MachineError::ConcurrentModelRound);
    }

    #[test]
    fn illegal_transition_is_reported() {
        let mut machine = machine();
        // Preparing -> DispatchingTools is not in the table.
        let error = machine
            .enter(&Action::Dispatch(Default::default()))
            .unwrap_err();
        assert!(matches!(error, MachineError::IllegalTransition { .. }));
    }

    #[test]
    fn completed_requires_a_terminal_event() {
        let mut machine = machine();
        machine.enter(&start_action()).unwrap();
        machine
            .enter(&Action::Finish {
                outcome: TurnOutcome::Completed,
            })
            .unwrap();
        assert_eq!(machine.state(), TurnState::Finalizing);
        assert_eq!(
            machine.complete().unwrap_err(),
            MachineError::MissingTerminalEvent
        );
    }

    #[test]
    fn terminal_then_complete_lands_completed() {
        let mut machine = machine();
        machine.enter(&start_action()).unwrap();
        machine
            .enter(&Action::Finish {
                outcome: TurnOutcome::Completed,
            })
            .unwrap();
        machine.mark_terminal().unwrap();
        machine.complete().unwrap();
        assert_eq!(machine.state(), TurnState::Completed);
        assert!(machine.terminal_written());
    }

    #[test]
    fn terminal_event_may_only_be_written_in_finalizing() {
        let mut machine = machine();
        machine.enter(&start_action()).unwrap();
        assert!(machine.mark_terminal().is_err());
    }

    #[test]
    fn abort_is_allowed_from_a_working_state() {
        let mut machine = machine();
        machine.enter(&start_action()).unwrap();
        machine.abort().unwrap();
        assert_eq!(machine.state(), TurnState::Aborted);
    }

    #[test]
    fn terminal_state_is_frozen() {
        let mut machine = machine();
        machine.abort().unwrap();
        assert_eq!(
            machine.enter(&start_action()).unwrap_err(),
            MachineError::AlreadyTerminal {
                state: TurnState::Aborted
            }
        );
    }

    #[test]
    fn usage_and_repeats_accumulate() {
        let mut machine = machine();
        machine.note_usage(&Usage::new(10, 5));
        machine.note_usage(&Usage::new(10, 5));
        assert_eq!(machine.progress().tokens_used, 30);

        let call = ToolCallRequest::new("c1", "read", serde_json::json!({ "path": "a" }));
        machine.record_calls(&[call.clone()]);
        machine.record_calls(&[call.clone()]);
        assert_eq!(machine.repeats_for(&[call]), vec![2]);
    }

    #[test]
    fn compaction_and_tool_calls_are_counted() {
        let mut machine = machine();
        machine.note_compaction(3);
        machine.note_tool_calls(2);
        assert_eq!(machine.progress().compactions, 1);
        assert_eq!(machine.progress().tool_calls, 2);
    }

    #[test]
    fn settle_maps_outcomes_to_terminal_states() {
        let mut completed = machine();
        completed.enter(&start_action()).unwrap();
        completed
            .enter(&Action::Finish {
                outcome: TurnOutcome::Completed,
            })
            .unwrap();
        completed.mark_terminal().unwrap();
        completed.settle(&TurnOutcome::Completed).unwrap();
        assert_eq!(completed.state(), TurnState::Completed);

        let mut aborted = machine();
        aborted.enter(&start_action()).unwrap();
        aborted.settle(&TurnOutcome::aborted("stop")).unwrap();
        assert_eq!(aborted.state(), TurnState::Aborted);
    }

    #[test]
    fn prompt_estimate_is_settable() {
        let mut machine = machine();
        machine.set_prompt_estimate(1234);
        assert_eq!(machine.progress().prompt_estimate, 1234);
    }
}
