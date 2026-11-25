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
        concurrency: None,
        steer_attachment_resolver: None,
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
        concurrency: None,
        steer_attachment_resolver: None,
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

// ---- Full-code-review regressions: AG-01 .. AG-05 -------------------------

/// Writer that fails on one chosen append (but still allows `finish`), so we
/// can prove an I/O error inside the loop does not orphan `turn_started`.
struct FailAfterWriter {
    inner: MemoryWriter,
    fail_at: usize,
    count: AtomicUsize,
}

#[async_trait::async_trait]
impl SessionWriter for FailAfterWriter {
    async fn append(&self, event: RuntimeEvent) -> Result<RuntimeEvent> {
        let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        if n == self.fail_at {
            return Err(ArkError::internal("injected append failure"));
        }
        self.inner.append(event).await
    }

    async fn finish(&self, outcome: &TurnOutcome) -> Result<RuntimeEvent> {
        self.inner.finish(outcome).await
    }
}

/// AG-01: any append failure after `turn_started` must still write exactly one
/// terminal event, and a second `run()` must be idempotent.
#[tokio::test]
async fn an_append_failure_still_writes_one_terminal_and_run_is_idempotent() {
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let writer = Arc::new(MemoryWriter::for_turn(events.clone()));
    let failing = Arc::new(FailAfterWriter {
        inner: MemoryWriter::for_turn(events.clone()),
        fail_at: 2, // turn_started succeeds, item_created fails
        count: AtomicUsize::new(0),
    });
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::clone(&failing) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: None,
    });
    reactor.push_user("hi");

    let outcome = reactor.run().await.expect("errors converge to an outcome");
    assert!(
        matches!(outcome, TurnOutcome::Failed { .. }),
        "an I/O failure must be recorded as Failed, not left running: {outcome:?}"
    );
    let recorded = failing.inner.recorded();
    let starts = recorded
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::TurnStarted { .. }))
        .count();
    let terminals = recorded
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::TurnFailed { .. }))
        .count();
    assert_eq!(starts, 1, "exactly one turn_started: {recorded:?}");
    assert_eq!(terminals, 1, "exactly one terminal: {recorded:?}");

    let before = recorded.len();
    let second = reactor
        .run()
        .await
        .expect("a terminal reactor is idempotent");
    assert!(matches!(second, TurnOutcome::Failed { .. }));
    assert_eq!(
        failing.inner.recorded().len(),
        before,
        "a repeated run() must not write a second turn_started or terminal"
    );
    let _ = writer; // keep the bound journal alive for the assertion above
}

/// Model whose stream never settles; the only way `pump` exits is the cancel
/// branch, which is exactly the race AG-02 is about.
struct NeverModel;

#[async_trait::async_trait]
impl ModelClient for NeverModel {
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
        std::future::pending::<Result<ModelResponse>>().await
    }
}

/// AG-02: a steer drained for a round that is cancelled before the model
/// consumed it must not be journaled and must remain in the queue.
#[tokio::test]
async fn a_cancelled_round_keeps_its_steering_instead_of_false_journaling() {
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let writer = Arc::new(MemoryWriter::for_turn(events.clone()));
    let queue = Arc::new(SteerQueue::new());
    assert!(queue.push("correction the model never saw"));
    let cancel = CancelToken::new();
    cancel.cancel(); // tripped before the first round
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NeverModel),
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::clone(&writer) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel,
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: None,
    })
    .with_steering(Arc::clone(&queue));

    let outcome = reactor.run().await.unwrap();
    assert!(matches!(outcome, TurnOutcome::Aborted { .. }));

    let recorded = writer.recorded();
    assert!(
        !recorded
            .iter()
            .any(|e| matches!(e, RuntimeEvent::TurnSteered { .. })),
        "the journal must not claim a correction the model never received: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|e| matches!(e, RuntimeEvent::TurnAborted { .. })),
        "the turn still converges to a terminal: {recorded:?}"
    );
    assert!(
        !queue.is_empty(),
        "a 202-accepted correction must stay queued for a retry"
    );
    let restored = queue.drain();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].text, "correction the model never saw");
}

/// Gate whose future never resolves: only the cancel branch of `select!` can
/// win, which is the AG-03 window under test.
struct PendingGates;

impl GateBroker for PendingGates {
    fn prepare_approval(
        &self,
        _request: ApprovalRequest,
    ) -> Result<crate::exec::GateFuture<crate::exec::ApprovalReply>> {
        Ok(Box::pin(async {
            std::future::pending::<Result<crate::exec::ApprovalReply>>().await
        }))
    }
    fn prepare_user_input(
        &self,
        _request: UserInputRequest,
    ) -> Result<crate::exec::GateFuture<String>> {
        Ok(Box::pin(async {
            std::future::pending::<Result<String>>().await
        }))
    }
}

/// AG-03: cancelling while a gate is pending must pair *every* declared call
/// with a finished row, including plain and sibling-gated calls.
#[tokio::test]
async fn a_cancel_during_a_gate_pairs_every_declared_call() {
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let writer = Arc::new(MemoryWriter::for_turn(events.clone()));
    let cancel = CancelToken::new();
    cancel.cancel();
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::clone(&writer) as Arc<dyn SessionWriter>,
        gates: Arc::new(PendingGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel,
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: None,
    });
    let calls = vec![
        ToolCallRequest::new("plain", "read", serde_json::json!({ "path": "a" })),
        ToolCallRequest::new("gated", "bash", serde_json::json!({ "cmd": "x" })),
        ToolCallRequest::new(
            "ask",
            "request_user_input",
            serde_json::json!({ "prompt": "p" }),
        ),
    ];
    reactor.record_assistant_turn("", &calls);
    reactor.pending_calls = calls.clone();
    reactor.plain_calls = vec![calls[0].clone()];
    reactor.pending_approvals = vec![ApprovalAsk::new(calls[1].clone(), "run x")];
    reactor.pending_user_inputs = vec![UserInputAsk::new(calls[2].clone(), "p")];

    let _ = reactor
        .ask_approval(calls[1].clone(), "run x".into())
        .await
        .unwrap();

    let recorded = writer.recorded();
    for id in ["plain", "gated", "ask"] {
        assert_eq!(
            finished_call_ids(&recorded)
                .iter()
                .filter(|c| c.as_str() == id)
                .count(),
            1,
            "{id} must have exactly one finished row: {recorded:?}"
        );
    }
    assert!(
        HistoryHygiene::new().is_legal(reactor.history()),
        "history must stay provider-legal after a gate cancel"
    );
}

/// AG-05: two calls sharing one provider-local `call_id` must both be asked
/// and both receive a finished row (the old `retain` removed both at once).
#[tokio::test]
async fn duplicate_call_ids_each_get_their_own_gate_and_result() {
    let model = Arc::new(CallsThenDoneModel {
        first_round: vec![
            ("dup", "bash", serde_json::json!({ "cmd": "a" })),
            ("dup", "bash", serde_json::json!({ "cmd": "b" })),
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
    assert_eq!(
        gates.approvals.load(Ordering::SeqCst),
        2,
        "both duplicate-id calls must open a gate"
    );
    assert_eq!(
        finished_call_ids(&writer.recorded())
            .iter()
            .filter(|id| id.as_str() == "dup")
            .count(),
        2,
        "both duplicate-id calls must be paired"
    );
    assert!(HistoryHygiene::new().is_legal(reactor.history()));
}

/// A later `ToolCallStarted` append failure must close every start that already
/// reached the journal, and must not execute any tool from the batch.
#[tokio::test]
async fn an_append_failure_closes_already_started_calls_without_executing() {
    struct CountingRunner(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl ToolRunner for CountingRunner {
        async fn execute(&self, _call: &ToolCallRequest) -> Result<ToolOutput> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::ok("executed"))
        }
    }

    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let failing = Arc::new(FailAfterWriter {
        inner: MemoryWriter::for_turn(events.clone()),
        fail_at: 2,
        count: AtomicUsize::new(0),
    });
    let executions = Arc::new(AtomicUsize::new(0));
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(CountingRunner(Arc::clone(&executions))),
        writer: Arc::clone(&failing) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: None,
    });
    let batch = vec![
        ToolCallRequest::new("c1", "read", serde_json::json!({ "path": "a" })),
        ToolCallRequest::new("c2", "read", serde_json::json!({ "path": "b" })),
    ];

    let result = reactor.run_batch(&batch).await;

    assert!(result.is_err(), "the original append error must propagate");
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    let recorded = failing.inner.recorded();
    let started: Vec<_> = recorded
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolCallStarted { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(started, ["c1"], "only the successful start is durable");
    let finished: Vec<_> = recorded
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolCallFinished {
                call_id, detail, ..
            } => Some((call_id.as_str(), detail.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(
        finished,
        [(
            "c1",
            Some("tool event persistence failed before completion")
        )],
        "the durable start must not remain orphaned: {recorded:?}"
    );
}

/// A failure while recording a synthetic refusal's optional storm event must
/// not leave its already-durable start open.
#[tokio::test]
async fn a_synthetic_result_append_failure_closes_its_started_call() {
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let failing = Arc::new(FailAfterWriter {
        inner: MemoryWriter::for_turn(events.clone()),
        fail_at: 2,
        count: AtomicUsize::new(0),
    });
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::clone(&failing) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: None,
    });
    let call = ToolCallRequest::new("storm", "bash", serde_json::json!({ "cmd": "x" }));
    let synthetic = SyntheticResult::new(&call, false, "suppressed").with_storm_count(3);
    let mut executions = Vec::new();

    let result = reactor.record_synthetic(&synthetic, &mut executions).await;

    assert!(result.is_err(), "the original append error must propagate");
    assert!(executions.is_empty(), "a failed record is not an execution");
    let recorded = failing.inner.recorded();
    assert_eq!(finished_call_ids(&recorded), ["storm"], "{recorded:?}");
    let detail = recorded.iter().find_map(|event| match event {
        RuntimeEvent::ToolCallFinished { detail, .. } => detail.as_deref(),
        _ => None,
    });
    assert_eq!(
        detail,
        Some("tool event persistence failed before completion")
    );
}

/// Cancellation convergence also persists a start before its finish; if that
/// finish append fails, the durable start must still be compensated.
#[tokio::test]
async fn a_cancelled_call_append_failure_closes_its_started_event() {
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let failing = Arc::new(FailAfterWriter {
        inner: MemoryWriter::for_turn(events.clone()),
        fail_at: 2,
        count: AtomicUsize::new(0),
    });
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::clone(&failing) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: None,
    });
    let call = ToolCallRequest::new("pending", "read", serde_json::json!({ "path": "a" }));
    reactor.record_assistant_turn("", std::slice::from_ref(&call));
    reactor.pending_calls = vec![call];

    let result = reactor.converge_pending_calls_on_cancel().await;

    assert!(result.is_err(), "the original append error must propagate");
    let recorded = failing.inner.recorded();
    assert_eq!(finished_call_ids(&recorded), ["pending"], "{recorded:?}");
    let detail = recorded.iter().find_map(|event| match event {
        RuntimeEvent::ToolCallFinished { detail, .. } => detail.as_deref(),
        _ => None,
    });
    assert_eq!(
        detail,
        Some("tool event persistence failed before completion")
    );
}

/// A failure while appending one result must close that call and every other
/// call whose start is already durable.
#[tokio::test]
async fn a_batch_result_append_failure_closes_all_durable_starts() {
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let failing = Arc::new(FailAfterWriter {
        inner: MemoryWriter::for_turn(events.clone()),
        fail_at: 3,
        count: AtomicUsize::new(0),
    });
    let tools = Arc::new(MapToolRunner::new().with("read", ToolOutput::ok("body")));
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools,
        writer: Arc::clone(&failing) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: None,
    });
    let batch = vec![
        ToolCallRequest::new("c1", "read", serde_json::json!({ "path": "a" })),
        ToolCallRequest::new("c2", "read", serde_json::json!({ "path": "b" })),
    ];

    let result = reactor.run_batch(&batch).await;

    assert!(result.is_err(), "the original append error must propagate");
    let recorded = failing.inner.recorded();
    assert_eq!(finished_call_ids(&recorded), ["c1", "c2"], "{recorded:?}");
    for event in recorded
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolCallFinished { .. }))
    {
        assert!(matches!(
            event,
            RuntimeEvent::ToolCallFinished { detail, .. }
                if detail.as_deref() == Some("tool event persistence failed before completion")
        ));
    }
}

/// A turn seeded with attachments must land the delivery record on the opening
/// `UserMessage` item, so history replay renders the same attachment chips the
/// live stream did (the `turn_started` audit record alone is not observable
/// after a reload).
#[tokio::test]
async fn the_opening_user_item_carries_the_attachment_outcomes() {
    let outcomes = vec![ark_contract::TurnAttachmentOutcome {
        attachment_id: "019689f4-8000-7000-8000-000000000001".parse().unwrap(),
        original_filename: Some("pic.png".into()),
        delivered: ark_contract::TurnAttachmentDelivery::Image,
        detail: None,
    }];
    let model = Arc::new(CallsThenDoneModel {
        first_round: vec![],
        rounds: AtomicUsize::new(0),
    });
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new())
        .with_attachments(vec![outcomes[0].attachment_id])
        .with_attachment_outcomes(outcomes.clone());
    let writer = Arc::new(MemoryWriter::for_turn(events.clone()));
    let mut reactor = Reactor::new(ReactorConfig {
        model,
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::clone(&writer) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: None,
    });
    reactor.push_user_message(ark_ports::ModelMessage::user("look at this"));

    let outcome = reactor.run().await.unwrap();
    assert_eq!(outcome, TurnOutcome::Completed);

    let user_items: Vec<_> = writer
        .recorded()
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ItemCreated { item, .. }
                if item.kind == ark_contract::item::ItemKind::UserMessage =>
            {
                Some(item.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(user_items.len(), 1, "{user_items:?}");
    assert_eq!(
        user_items[0].attachments.as_deref(),
        Some(outcomes.as_slice()),
        "the item must carry the same outcomes recorded on turn_started"
    );
    let started = writer
        .recorded()
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::TurnStarted {
                attachment_outcomes,
                ..
            } => Some(attachment_outcomes.clone()),
            _ => None,
        })
        .expect("turn_started is journaled before the user item");
    assert_eq!(
        started, outcomes,
        "the audit record on the event is unchanged"
    );
}

/// A steering message is a mid-turn correction, not a seeded attachment
/// carrier: its item must stay bare even when the turn itself has attachments.
#[tokio::test]
async fn a_steering_item_does_not_inherit_attachment_outcomes() {
    let outcomes = vec![ark_contract::TurnAttachmentOutcome {
        attachment_id: "019689f4-8000-7000-8000-000000000002".parse().unwrap(),
        original_filename: Some("pic.png".into()),
        delivered: ark_contract::TurnAttachmentDelivery::Image,
        detail: None,
    }];
    let model = Arc::new(CallsThenDoneModel {
        first_round: vec![],
        rounds: AtomicUsize::new(0),
    });
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new())
        .with_attachments(vec![outcomes[0].attachment_id])
        .with_attachment_outcomes(outcomes);
    let writer = Arc::new(MemoryWriter::for_turn(events.clone()));
    let mut reactor = Reactor::new(ReactorConfig {
        model,
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::clone(&writer) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        concurrency: None,
        steer_attachment_resolver: None,
    })
    .with_steering(Arc::new({
        let queue = SteerQueue::new();
        assert!(queue.push("late correction"));
        queue
    }));

    let outcome = reactor.run().await.unwrap();
    assert_eq!(outcome, TurnOutcome::Completed);

    let user_items: Vec<_> = writer
        .recorded()
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ItemCreated { item, .. }
                if item.kind == ark_contract::item::ItemKind::UserMessage =>
            {
                Some(item.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        user_items.len(),
        1,
        "only the steering message is a user item"
    );
    assert!(
        user_items[0].text.as_deref() == Some("late correction"),
        "the journaled item is the steering message: {user_items:?}"
    );
    assert!(
        user_items[0].attachments.is_none(),
        "a steering correction must not claim the turn's attachments"
    );
}
