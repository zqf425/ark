//! Regression tests for the full-code-review findings: A1 (mixed rounds drop
//! plain calls), A2 (multi-ask rounds), A3 (cancelled tool batches). Kept in
//! their own file so `reactor.rs` stays within the file-size governance limit
//! (AGENTS §4); declared inside `reactor::tests`, so `use super::*` reaches
//! the shared doubles.

use super::*;

use crate::exec::ApprovalReply;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Sandbox double: `bash` always needs a human, everything else is read-only.
struct BashNeedsApproval;

impl SandboxPolicy for BashNeedsApproval {
    fn decide(&self, tool: &str, _args: &ToolArgs) -> SandboxDecision {
        if tool == "bash" {
            SandboxDecision::NeedsApproval
        } else {
            SandboxDecision::ReadOnly
        }
    }
}

/// Gate double that answers every gate immediately and counts how many of
/// each kind were opened.
#[derive(Default)]
struct AutoGates {
    approvals: AtomicUsize,
    asks: AtomicUsize,
}

impl GateBroker for AutoGates {
    fn prepare_approval(
        &self,
        _request: ApprovalRequest,
    ) -> Result<crate::exec::GateFuture<ApprovalReply>> {
        self.approvals.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(async { Ok(ApprovalReply::Approved) }))
    }

    fn prepare_user_input(
        &self,
        _request: UserInputRequest,
    ) -> Result<crate::exec::GateFuture<String>> {
        self.asks.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(async { Ok("answered".to_string()) }))
    }
}

/// Model that emits one fixed round of tool calls, then plain completions.
struct CallsThenDoneModel {
    first_round: Vec<(&'static str, &'static str, serde_json::Value)>,
    rounds: AtomicUsize,
}

#[async_trait::async_trait]
impl ModelClient for CallsThenDoneModel {
    fn provider_id(&self) -> &str {
        "test"
    }

    fn model(&self) -> &str {
        "test"
    }

    async fn stream(
        &self,
        _request: ModelRequest,
        _sink: mpsc::Sender<ModelChunk>,
    ) -> Result<ModelResponse> {
        if self.rounds.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(ModelResponse {
                text: String::new(),
                tool_calls: self
                    .first_round
                    .iter()
                    .map(|(id, tool, args)| (id.to_string(), tool.to_string(), args.clone()))
                    .collect(),
                usage: Usage::default(),
            });
        }
        Ok(ModelResponse::default())
    }
}

/// Runner that never answers: simulates a tool stuck mid-flight while the
/// user interrupts the turn.
struct HangingRunner;

#[async_trait::async_trait]
impl ToolRunner for HangingRunner {
    async fn execute(&self, _call: &ToolCallRequest) -> Result<ToolOutput> {
        std::future::pending::<Result<ToolOutput>>().await
    }
}

fn regression_reactor(
    model: Arc<dyn ModelClient>,
    gates: Arc<dyn GateBroker>,
    tools: Arc<dyn ToolRunner>,
    sandbox: Arc<dyn SandboxPolicy>,
) -> (Reactor, Arc<MemoryWriter>) {
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    // Bound to the turn so the completed run can write its terminal event.
    let writer = Arc::new(MemoryWriter::for_turn(events.clone()));
    let reactor = Reactor::new(ReactorConfig {
        model,
        tools,
        writer: Arc::clone(&writer) as Arc<dyn SessionWriter>,
        gates,
        sandbox,
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
    });
    (reactor, writer)
}

fn finished_call_ids(events: &[RuntimeEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolCallFinished { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect()
}

/// A1: a round with one approval-gated call and one plain call must not drop
/// the plain call. Its id is already in the assistant row, so a missing
/// result row poisons the history for the next model round.
#[tokio::test]
async fn mixed_round_dispatches_plain_calls_after_the_approval_chain() {
    let model = Arc::new(CallsThenDoneModel {
        first_round: vec![
            ("call-1", "bash", serde_json::json!({ "cmd": "dir" })),
            ("call-2", "read", serde_json::json!({ "path": "a.txt" })),
        ],
        rounds: AtomicUsize::new(0),
    });
    let tools = Arc::new(
        MapToolRunner::new()
            .with("bash", ToolOutput::ok("ran"))
            .with("read", ToolOutput::ok("body")),
    );
    let (mut reactor, writer) = regression_reactor(
        model,
        Arc::new(AutoGates::default()),
        tools,
        Arc::new(BashNeedsApproval),
    );
    reactor.push_user("hi");

    let outcome = reactor.run().await.unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    let finished = finished_call_ids(&writer.recorded());
    assert!(
        finished.iter().any(|id| id == "call-1"),
        "approved call finished: {finished:?}"
    );
    assert!(
        finished.iter().any(|id| id == "call-2"),
        "the plain call of a mixed round must not be dropped (A1): {finished:?}"
    );
    // Every call id of the assistant row has a paired tool result row.
    let history = reactor.history();
    let assistant = history
        .iter()
        .find(|message| message.role == ModelRole::Assistant && !message.tool_calls.is_empty())
        .expect("assistant tool-call row");
    for call in &assistant.tool_calls {
        assert!(
            history.iter().any(|message| message.role == ModelRole::Tool
                && message.tool_call_id.as_deref() == Some(call.call_id.as_str())),
            "missing tool result row for {}",
            call.call_id
        );
    }
    assert!(
        HistoryHygiene::new().is_legal(history),
        "history must stay provider-legal"
    );
}

/// A2: several `request_user_input` calls in one round are each presented and
/// answered in their own round, and an approval ask of the same round is not
/// skipped.
#[tokio::test]
async fn every_ask_of_a_round_is_presented_and_answered_before_approvals() {
    let model = Arc::new(CallsThenDoneModel {
        first_round: vec![
            (
                "ask-1",
                "request_user_input",
                serde_json::json!({ "prompt": "first?" }),
            ),
            (
                "ask-2",
                "request_user_input",
                serde_json::json!({ "prompt": "second?" }),
            ),
            ("call-3", "bash", serde_json::json!({ "cmd": "dir" })),
        ],
        rounds: AtomicUsize::new(0),
    });
    let gates = Arc::new(AutoGates::default());
    let tools = Arc::new(MapToolRunner::new().with("bash", ToolOutput::ok("ran")));
    let (mut reactor, writer) = regression_reactor(
        model,
        Arc::clone(&gates) as Arc<dyn GateBroker>,
        tools,
        Arc::new(BashNeedsApproval),
    );
    reactor.push_user("hi");

    let outcome = reactor.run().await.unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    let recorded = writer.recorded();
    let prompts: Vec<String> = recorded
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::UserInputRequested { prompt, .. } => Some(prompt.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        prompts,
        vec!["first?".to_string(), "second?".to_string()],
        "every ask is presented exactly once, in model order"
    );
    let resolved = recorded
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::UserInputResolved { .. }))
        .count();
    assert_eq!(resolved, 2, "every ask gets its own result row");
    let finished = finished_call_ids(&recorded);
    for call_id in ["ask-1", "ask-2", "call-3"] {
        assert!(
            finished.iter().any(|id| id == call_id),
            "{call_id} must have a finished row: {finished:?}"
        );
    }
    assert_eq!(
        gates.asks.load(Ordering::SeqCst),
        2,
        "each ask opens its own gate"
    );
    assert_eq!(
        gates.approvals.load(Ordering::SeqCst),
        1,
        "the approval ask of the same round is not skipped"
    );
    assert!(
        HistoryHygiene::new().is_legal(reactor.history()),
        "history must stay provider-legal"
    );
}

/// A3: a batch interrupted mid-flight must close every started call with a
/// `finished` row and a paired tool history entry, instead of leaving the
/// journal hanging on `started`.
#[tokio::test]
async fn a_cancelled_batch_finishes_every_started_call() {
    let writer = Arc::new(MemoryWriter::new());
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let cancel = CancelToken::new();
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(HangingRunner),
        writer: writer.clone(),
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: cancel.clone(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
    });
    let batch = vec![
        ToolCallRequest::new("c1", "read", serde_json::json!({ "path": "a" })),
        ToolCallRequest::new("c2", "read", serde_json::json!({ "path": "b" })),
    ];
    // Mirror the real round: the assistant row already carries these call ids
    // when a batch runs, and the hygiene check below relies on it.
    reactor.record_assistant_turn("", &batch);

    // The cancel trips while the batch is in flight; the runner never answers,
    // so the select can only resolve through the cancellation branch.
    cancel.cancel();
    let (executions, interrupted) = reactor.run_batch(&batch).await.unwrap();

    assert!(interrupted);
    assert!(executions.is_empty());
    let recorded = writer.recorded();
    let started = recorded
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolCallStarted { .. }))
        .count();
    assert_eq!(started, 2, "both calls were started before the cancel");
    for call_id in ["c1", "c2"] {
        let event = recorded
            .iter()
            .find_map(|event| match event {
                RuntimeEvent::ToolCallFinished {
                    call_id: id,
                    ok,
                    detail,
                    ..
                } if id == call_id => Some((*ok, detail.clone())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{call_id} must have a finished row"));
        assert!(!event.0, "the interrupted call did not complete");
        assert_eq!(
            event.1.as_deref(),
            Some("cancelled before the call completed")
        );
    }
    let history = reactor.history();
    for call_id in ["c1", "c2"] {
        assert!(
            history.iter().any(|message| message.role == ModelRole::Tool
                && message.tool_call_id.as_deref() == Some(call_id)),
            "the cancelled call keeps a paired tool history row"
        );
    }
    assert!(
        HistoryHygiene::new().is_legal(history),
        "history must stay provider-legal"
    );
}
