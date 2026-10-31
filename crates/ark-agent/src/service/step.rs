//! The pure transition function `(TurnState, TurnInput) -> Action` (design §7.1).
//!
//! No I/O, no clock, no randomness, no mutation: everything the decision needs
//! arrives inside [`TurnInput`]. That is what makes the whole turn logic
//! unit-testable without HTTP, a provider, or a mock model.
//!
//! Each helper stays well under the 80-line function budget; the top-level
//! [`step`] only dispatches on the `(state, input)` pair.

use crate::domain::action::{Action, DispatchPlan, SyntheticResult, TurnOutcome};
use crate::domain::input::{ApprovalAsk, InputKind, Progress, TurnInput};
use crate::domain::state::TurnState;
use crate::domain::tool::ToolCallRequest;
use crate::policy::budget::{BudgetVerdict, TurnBudget};
use crate::policy::parallel::ParallelPolicy;
use crate::policy::storm::{suppression_message, StormBreaker};

/// Decide the next action. Total over `(state, input)`: an impossible pair
/// yields [`Action::Invalid`] instead of silently doing nothing.
pub fn step(state: TurnState, input: &TurnInput) -> Action {
    if let InputKind::Interrupt { reason } = &input.kind {
        return finish(TurnOutcome::aborted(reason.clone()));
    }
    match (state, &input.kind) {
        (TurnState::Preparing, InputKind::Start) => begin(input),
        (TurnState::AwaitingModel, InputKind::ModelSettled { .. }) => settle_model(input),
        (TurnState::AwaitingModel, InputKind::ModelFailed { message }) => {
            finish(TurnOutcome::failed(message.clone()))
        }
        (TurnState::DispatchingTools, InputKind::ToolsSettled { .. }) => after_tools(input),
        (TurnState::AwaitingApproval, InputKind::GateResolved { .. }) => gate_resolved(input),
        (TurnState::AwaitingUserInput, InputKind::UserInputResolved { .. }) => {
            after_user_input(input)
        }
        (TurnState::Compacting, InputKind::CompactionSettled { .. }) => after_compaction(input),
        (state, kind) => invalid(state, kind),
    }
}

fn invalid(state: TurnState, kind: &InputKind) -> Action {
    Action::Invalid {
        reason: format!(
            "no transition for state `{}` and input `{}`",
            state.as_str(),
            kind_name(kind)
        ),
    }
}

fn kind_name(kind: &InputKind) -> &'static str {
    match kind {
        InputKind::Start => "start",
        InputKind::ModelSettled { .. } => "model_settled",
        InputKind::ModelFailed { .. } => "model_failed",
        InputKind::ToolsSettled { .. } => "tools_settled",
        InputKind::GateResolved { .. } => "gate_resolved",
        InputKind::UserInputResolved { .. } => "user_input_resolved",
        InputKind::CompactionSettled { .. } => "compaction_settled",
        InputKind::Interrupt { .. } => "interrupt",
    }
}

fn finish(outcome: TurnOutcome) -> Action {
    Action::Finish { outcome }
}

/// `Preparing`: compact first when the prefix no longer fits, else call the model.
fn begin(input: &TurnInput) -> Action {
    let budget = TurnBudget::new(input.limits);
    if budget.needs_compaction(input.progress.prompt_estimate)
        && budget.compaction_available(&input.progress)
    {
        return Action::Compact {
            keep_recent: input.limits.keep_recent_messages,
        };
    }
    call_model(input)
}

/// Shared "may we call the model" gate. Every round passes through here, so the
/// budget reason is identical wherever it is reported.
fn call_model(input: &TurnInput) -> Action {
    match TurnBudget::new(input.limits).check(&input.progress) {
        BudgetVerdict::Allow => Action::CallModel {
            round: input.progress.rounds + 1,
        },
        verdict => finish(TurnOutcome::failed(
            verdict.message().unwrap_or("turn budget exhausted"),
        )),
    }
}

fn settle_model(input: &TurnInput) -> Action {
    let InputKind::ModelSettled {
        tool_calls,
        repeats,
        ..
    } = &input.kind
    else {
        return invalid(TurnState::AwaitingModel, &input.kind);
    };
    if tool_calls.is_empty() {
        return finish(TurnOutcome::Completed);
    }
    // `request_user_input` never dispatches: the reactor intercepted it before
    // the plan was built, so the first ask blocks the turn on the human.
    if let Some(ask) = input.user_inputs.first() {
        return Action::AskUser {
            call: ask.call.clone(),
            prompt: ask.prompt.clone(),
        };
    }
    let first_gate = input.approvals.first().cloned();
    match first_gate {
        Some(ask) => Action::RequestApproval {
            call: ask.call,
            summary: ask.summary,
        },
        None => dispatch(tool_calls, repeats, input, DispatchYield::default()),
    }
}

/// Accumulated results from earlier denials, carried through the gate sequence.
#[derive(Debug, Default, Clone)]
struct DispatchYield {
    synthetic: Vec<SyntheticResult>,
    extra: Vec<ToolCallRequest>,
}

fn dispatch(
    calls: &[ToolCallRequest],
    repeats: &[u32],
    input: &TurnInput,
    carry: DispatchYield,
) -> Action {
    let storm = StormBreaker::new(input.limits.max_identical_calls).check(calls, repeats);
    let mut synthetic = carry.synthetic;
    for suppressed in &storm.suppressed {
        synthetic.push(
            SyntheticResult::new(
                &suppressed.call,
                false,
                suppression_message(&suppressed.call.tool, suppressed.count),
            )
            .with_storm_count(suppressed.count),
        );
    }
    let mut runnable = storm.allowed;
    runnable.extend(carry.extra);
    let batches = ParallelPolicy::default().plan(&runnable);
    Action::Dispatch(DispatchPlan {
        batches,
        synthetic_results: synthetic,
    })
}

/// A gate was resolved: ask the next one, or run what is now permitted.
fn gate_resolved(input: &TurnInput) -> Action {
    let InputKind::GateResolved {
        call,
        approved,
        reason,
        next_ask,
        pending_dispatch,
        synthetic_so_far,
        repeats,
    } = &input.kind
    else {
        return invalid(TurnState::AwaitingApproval, &input.kind);
    };
    let mut carry = DispatchYield {
        synthetic: synthetic_so_far.clone(),
        extra: Vec::new(),
    };
    if !approved {
        let message = if reason.trim().is_empty() {
            "denied by the approval gate".to_string()
        } else {
            format!("denied by the approval gate: {reason}")
        };
        carry
            .synthetic
            .push(SyntheticResult::new(call, false, message));
    } else {
        carry.extra.push(call.clone());
    }
    if let Some(next) = next_ask {
        return Action::RequestApproval {
            call: next.call.clone(),
            summary: next.summary.clone(),
        };
    }
    dispatch(pending_dispatch, repeats, input, carry)
}

fn after_tools(input: &TurnInput) -> Action {
    call_model(input)
}

/// The user answered: the answer already became the tool result, so the turn
/// simply continues with the next model round.
fn after_user_input(input: &TurnInput) -> Action {
    call_model(input)
}

fn after_compaction(input: &TurnInput) -> Action {
    let mut next = input.clone();
    next.progress.prompt_estimate = 0;
    call_model(&next)
}

/// Convenience for the machine: build a `Progress` with the round advanced.
pub fn advance_round(mut progress: Progress) -> Progress {
    progress.rounds += 1;
    progress
}

/// Convenience for the machine: remaining approval asks after the first.
pub fn rest_of(asks: &[ApprovalAsk]) -> Vec<ApprovalAsk> {
    asks.iter().skip(1).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::input::InputKind;
    use crate::domain::limits::TurnLimits;
    use ark_common::Usage;

    fn input(progress: Progress, kind: InputKind) -> TurnInput {
        TurnInput::new(TurnLimits::default(), progress, kind)
    }

    fn call(id: &str, tool: &str, path: &str) -> ToolCallRequest {
        ToolCallRequest::new(id, tool, serde_json::json!({ "path": path }))
    }

    fn settled(calls: Vec<ToolCallRequest>) -> InputKind {
        let repeats = vec![0; calls.len()];
        InputKind::ModelSettled {
            text: String::new(),
            tool_calls: calls,
            repeats,
            usage: Usage::default(),
        }
    }

    #[test]
    fn intercepted_user_input_becomes_ask_user_with_the_prompt() {
        let ask = crate::domain::input::UserInputAsk::new(
            ToolCallRequest::new("c1", "request_user_input", serde_json::json!({})),
            "which branch?",
        );
        let mut turn = input(Progress::default(), settled(vec![ask.call.clone()]));
        turn = turn.with_user_inputs(vec![ask]);
        let action = step(TurnState::AwaitingModel, &turn);
        assert_eq!(
            action,
            Action::AskUser {
                call: ToolCallRequest::new("c1", "request_user_input", serde_json::json!({})),
                prompt: "which branch?".into(),
            }
        );
    }

    #[test]
    fn user_input_resolved_continues_with_the_next_model_round() {
        let call = ToolCallRequest::new("c1", "request_user_input", serde_json::json!({}));
        let turn = input(
            Progress::default(),
            InputKind::UserInputResolved {
                call: call.clone(),
                answer: "main".into(),
            },
        );
        let action = step(TurnState::AwaitingUserInput, &turn);
        assert_eq!(action, Action::CallModel { round: 1 });
    }

    #[test]
    fn plain_tools_are_not_blocked_by_the_user_input_gate() {
        let turn = input(Progress::default(), settled(vec![call("c1", "bash", "a")]));
        let action = step(TurnState::AwaitingModel, &turn);
        assert!(matches!(action, Action::Dispatch(_)), "got {action:?}");
    }

    #[test]
    fn start_calls_the_model() {
        let action = step(
            TurnState::Preparing,
            &input(Progress::default(), InputKind::Start),
        );
        assert_eq!(action, Action::CallModel { round: 1 });
    }

    #[test]
    fn start_compacts_when_the_prefix_is_too_large() {
        let progress = Progress {
            prompt_estimate: u64::MAX / 2,
            ..Progress::default()
        };
        let action = step(TurnState::Preparing, &input(progress, InputKind::Start));
        assert!(matches!(action, Action::Compact { .. }));
    }

    #[test]
    fn start_fails_when_context_is_full_and_compaction_spent() {
        let limits = TurnLimits {
            max_compactions: 0,
            context_limit_tokens: 10,
            ..TurnLimits::default()
        };
        let progress = Progress {
            prompt_estimate: 100,
            ..Progress::default()
        };
        let action = step(
            TurnState::Preparing,
            &TurnInput::new(limits, progress, InputKind::Start),
        );
        assert!(matches!(
            action,
            Action::Finish {
                outcome: TurnOutcome::Failed { .. }
            }
        ));
    }

    #[test]
    fn settled_without_tools_completes() {
        let action = step(
            TurnState::AwaitingModel,
            &input(Progress::default(), settled(vec![])),
        );
        assert_eq!(
            action,
            Action::Finish {
                outcome: TurnOutcome::Completed
            }
        );
    }

    #[test]
    fn settled_with_tools_dispatches_a_plan() {
        let kind = settled(vec![call("c1", "read", "a.rs"), call("c2", "read", "b.rs")]);
        let action = step(TurnState::AwaitingModel, &input(Progress::default(), kind));
        match action {
            Action::Dispatch(plan) => {
                assert_eq!(
                    plan.batches.len(),
                    1,
                    "two reads of different files share a batch"
                );
                assert_eq!(plan.total_calls(), 2);
                assert!(plan.synthetic_results.is_empty());
            }
            other => panic!("expected dispatch, got {other:?}"),
        }
    }

    #[test]
    fn stormed_calls_are_dispatched_as_synthetic_results() {
        let limits = TurnLimits {
            max_identical_calls: 1,
            ..TurnLimits::default()
        };
        let calls = vec![call("c9", "read", "a.rs")];
        let kind = InputKind::ModelSettled {
            text: String::new(),
            tool_calls: calls,
            repeats: vec![5],
            usage: Usage::default(),
        };
        let action = step(
            TurnState::AwaitingModel,
            &TurnInput::new(limits, Progress::default(), kind),
        );
        match action {
            Action::Dispatch(plan) => {
                assert!(plan.batches.is_empty());
                assert_eq!(plan.synthetic_results.len(), 1);
                assert!(!plan.synthetic_results[0].ok);
            }
            other => panic!("expected dispatch, got {other:?}"),
        }
    }

    #[test]
    fn settled_with_identical_calls_suppresses_the_excess_in_one_batch() {
        let calls = (1..=6)
            .map(|id| call(&format!("c{id}"), "ls", "."))
            .collect::<Vec<_>>();
        let action = step(
            TurnState::AwaitingModel,
            &input(Progress::default(), settled(calls)),
        );

        match action {
            Action::Dispatch(plan) => {
                assert_eq!(plan.synthetic_results.len(), 3);
                assert_eq!(plan.batches.iter().map(Vec::len).sum::<usize>(), 3);
                assert_eq!(plan.total_calls(), 6);
                assert_eq!(
                    plan.synthetic_results
                        .iter()
                        .map(|result| result.storm_count)
                        .collect::<Vec<_>>(),
                    vec![Some(4), Some(5), Some(6)]
                );
            }
            other => panic!("expected dispatch, got {other:?}"),
        }
    }

    #[test]
    fn settled_with_different_call_arguments_keeps_all_calls_runnable() {
        let calls = vec![
            call("c1", "ls", "a"),
            call("c2", "ls", "b"),
            call("c3", "ls", "c"),
            call("c4", "ls", "d"),
        ];
        let action = step(
            TurnState::AwaitingModel,
            &input(Progress::default(), settled(calls)),
        );

        match action {
            Action::Dispatch(plan) => {
                assert!(plan.synthetic_results.is_empty());
                assert_eq!(plan.batches.iter().map(Vec::len).sum::<usize>(), 4);
            }
            other => panic!("expected dispatch, got {other:?}"),
        }
    }

    #[test]
    fn approvals_are_asked_before_dispatch() {
        let ask = ApprovalAsk::new(call("c1", "bash", "x"), "run a command");
        let input = TurnInput::new(
            TurnLimits::default(),
            Progress::default(),
            settled(vec![call("c1", "bash", "x")]),
        )
        .with_approvals(vec![ask]);
        let action = step(TurnState::AwaitingModel, &input);
        assert!(matches!(action, Action::RequestApproval { .. }));
    }

    #[test]
    fn approved_gate_dispatches_the_call() {
        let kind = InputKind::GateResolved {
            call: call("c1", "bash", "x"),
            approved: true,
            reason: String::new(),
            next_ask: None,
            pending_dispatch: vec![],
            synthetic_so_far: vec![],
            repeats: vec![0],
        };
        let action = step(
            TurnState::AwaitingApproval,
            &input(Progress::default(), kind),
        );
        match action {
            Action::Dispatch(plan) => assert_eq!(plan.total_calls(), 1),
            other => panic!("expected dispatch, got {other:?}"),
        }
    }

    #[test]
    fn denied_gate_yields_a_synthetic_failure_and_runs_the_rest() {
        let kind = InputKind::GateResolved {
            call: call("c1", "bash", "x"),
            approved: false,
            reason: "not allowed".into(),
            next_ask: None,
            pending_dispatch: vec![call("c2", "read", "a.rs")],
            synthetic_so_far: vec![],
            repeats: vec![0],
        };
        let action = step(
            TurnState::AwaitingApproval,
            &input(Progress::default(), kind),
        );
        match action {
            Action::Dispatch(plan) => {
                assert_eq!(plan.synthetic_results.len(), 1);
                assert!(plan.synthetic_results[0].content.contains("not allowed"));
                assert_eq!(plan.batches.len(), 1);
            }
            other => panic!("expected dispatch, got {other:?}"),
        }
    }

    #[test]
    fn gate_chain_asks_the_next_gate() {
        let next = ApprovalAsk::new(call("c2", "bash", "y"), "second");
        let kind = InputKind::GateResolved {
            call: call("c1", "bash", "x"),
            approved: true,
            reason: String::new(),
            next_ask: Some(next),
            pending_dispatch: vec![],
            synthetic_so_far: vec![],
            repeats: vec![0],
        };
        let action = step(
            TurnState::AwaitingApproval,
            &input(Progress::default(), kind),
        );
        assert!(matches!(action, Action::RequestApproval { .. }));
    }

    #[test]
    fn interrupt_aborts_from_any_state() {
        for state in [
            TurnState::Preparing,
            TurnState::AwaitingModel,
            TurnState::DispatchingTools,
        ] {
            let action = step(
                state,
                &input(
                    Progress::default(),
                    InputKind::Interrupt {
                        reason: "user".into(),
                    },
                ),
            );
            assert_eq!(
                action,
                Action::Finish {
                    outcome: TurnOutcome::aborted("user")
                }
            );
        }
    }

    #[test]
    fn model_failure_finishes_failed() {
        let kind = InputKind::ModelFailed {
            message: "502".into(),
        };
        let action = step(TurnState::AwaitingModel, &input(Progress::default(), kind));
        assert!(matches!(
            action,
            Action::Finish {
                outcome: TurnOutcome::Failed { .. }
            }
        ));
    }

    #[test]
    fn tools_settled_calls_the_next_round() {
        let kind = InputKind::ToolsSettled { executions: vec![] };
        let action = step(
            TurnState::DispatchingTools,
            &input(Progress::default(), kind),
        );
        assert_eq!(action, Action::CallModel { round: 1 });
    }

    #[test]
    fn tools_settled_respects_the_round_budget() {
        let progress = Progress {
            rounds: 4,
            ..Progress::default()
        };
        let kind = InputKind::ToolsSettled { executions: vec![] };
        let action = step(
            TurnState::DispatchingTools,
            &TurnInput::new(TurnLimits::tiny(), progress, kind),
        );
        assert!(matches!(
            action,
            Action::Finish {
                outcome: TurnOutcome::Failed { .. }
            }
        ));
    }

    #[test]
    fn compaction_settles_back_into_a_model_call() {
        let kind = InputKind::CompactionSettled { removed_items: 3 };
        let action = step(TurnState::Compacting, &input(Progress::default(), kind));
        assert_eq!(action, Action::CallModel { round: 1 });
    }

    #[test]
    fn impossible_pairs_are_reported_as_invalid() {
        let action = step(
            TurnState::Completed,
            &input(Progress::default(), InputKind::Start),
        );
        assert!(action.is_invalid());
        let action = step(
            TurnState::Preparing,
            &input(
                Progress::default(),
                InputKind::ToolsSettled { executions: vec![] },
            ),
        );
        assert!(action.is_invalid());
    }

    #[test]
    fn advance_round_and_rest_of_helpers() {
        assert_eq!(advance_round(Progress::default()).rounds, 1);
        let asks = vec![
            ApprovalAsk::new(call("c1", "bash", "x"), "a"),
            ApprovalAsk::new(call("c2", "bash", "y"), "b"),
        ];
        assert_eq!(rest_of(&asks).len(), 1);
        assert_eq!(rest_of(&[]).len(), 0);
    }
}
