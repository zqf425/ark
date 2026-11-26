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
    // v2 (requirements/parallel-tools-v2 §2): the batch always settles every
    // slot — a hanging runner delivers nothing, so both slots get synthetic
    // cancel rows and the executions vector is non-empty (it would have been
    // empty in v1, which aborted the whole batch instead of settling it).
    assert_eq!(executions.len(), 2);
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

fn assert_cancelled_batch_results(
    reactor: &Reactor,
    writer: &MemoryWriter,
    executions: &[ToolExecution],
    interrupted: bool,
) {
    assert!(interrupted);
    assert_eq!(
        executions.len(),
        2,
        "both calls settle: one real, one synthetic"
    );
    let fast = executions
        .iter()
        .find(|execution| execution.call_id == "fast")
        .expect("the fast call keeps its execution");
    assert!(fast.ok, "the settled call keeps its real result");
    assert_eq!(fast.content, "fast result");
    assert_eq!(fast.detail, None, "a kept result is not marked cancelled");
    let slow = executions
        .iter()
        .find(|execution| execution.call_id == "slow")
        .expect("the hanging call still settles synthetically");
    assert!(!slow.ok);
    assert_eq!(
        slow.detail.as_deref(),
        Some("cancelled before the call completed")
    );
    let recorded = writer.recorded();
    let fast_finished = recorded
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolCallFinished { call_id, .. } if call_id == "fast"))
        .count();
    assert_eq!(
        fast_finished, 1,
        "exactly one finished row for the kept call"
    );
    let history = reactor.history();
    assert_eq!(
        history
            .iter()
            .filter(|message| message.role == ModelRole::Tool)
            .map(|message| message.tool_call_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("slow"), Some("fast")],
        "history rows follow submission order even though completion order differs"
    );
    assert!(
        HistoryHygiene::new().is_legal(history),
        "history must stay provider-legal"
    );
}

/// v2 (requirements/parallel-tools-v2 §2.3): a batch cancelled after one call
/// already delivered its result keeps the real result and only synthesizes a
/// cancellation row for the call that never settled.
#[tokio::test]
async fn a_cancelled_batch_keeps_results_that_reached_settlement() {
    struct FirstSlowSecondFast;
    #[async_trait::async_trait]
    impl ToolRunner for FirstSlowSecondFast {
        async fn execute(&self, call: &ToolCallRequest) -> Result<ToolOutput> {
            if call.call_id == "slow" {
                // Never settles: the cancel must abort this future.
                std::future::pending::<Result<ToolOutput>>().await
            } else {
                Ok(ToolOutput::ok("fast result"))
            }
        }
    }
    let writer = Arc::new(MemoryWriter::new());
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let cancel = CancelToken::new();
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(FirstSlowSecondFast),
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
        ToolCallRequest::new("slow", "read", serde_json::json!({ "path": "a" })),
        ToolCallRequest::new("fast", "read", serde_json::json!({ "path": "b" })),
    ];
    reactor.record_assistant_turn("", &batch);

    // Start the batch first so the fast call settles into the channel, then
    // cancel from this task while the slow call hangs. `run_batch` takes
    // `&mut reactor`, so it runs inside the spawned task; the cancel token is
    // cloned into the reactor and tripped here afterwards.
    let runner_handle = tokio::spawn(async move {
        let result = reactor.run_batch(&batch).await;
        (reactor, result)
    });
    // Give the fast call a beat to settle into the channel, then cancel.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.cancel();
    let (reactor, batch_result) = runner_handle.await.unwrap();
    let (executions, interrupted) = batch_result.unwrap();
    assert_cancelled_batch_results(&reactor, &writer, &executions, interrupted);
}

/// §2.4 convergence: a cancel that fires only after every call of the batch
/// settled (double-ready select race) must NOT abort the turn — the batch is
/// fully successful, so the reactor returns a normal completion and the drive
/// loop's own cancel handling ends the turn.
#[tokio::test]
async fn a_cancel_after_every_call_settled_does_not_interrupt() {
    struct Quick;
    #[async_trait::async_trait]
    impl ToolRunner for Quick {
        async fn execute(&self, _call: &ToolCallRequest) -> Result<ToolOutput> {
            Ok(ToolOutput::ok("done"))
        }
    }
    let writer = Arc::new(MemoryWriter::new());
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let cancel = CancelToken::new();
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(Quick),
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
    reactor.record_assistant_turn("", &batch);

    // Cancel fires after both calls settled: the select! race may pick the
    // cancel branch, but with no unsettled slots the batch must converge.
    // The batch runs in a spawned task so it is polled (current-thread
    // runtime) before the cancel trips.
    let runner = tokio::spawn(async move {
        let result = reactor.run_batch(&batch).await;
        (reactor, result)
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    cancel.cancel();
    let (_reactor, batch_result) = runner.await.unwrap();
    let (executions, interrupted) = batch_result.unwrap();

    assert!(
        !interrupted,
        "a fully-settled batch never reports interruption"
    );
    assert_eq!(executions.len(), 2);
    assert!(executions.iter().all(|execution| execution.ok));
    for execution in &executions {
        assert_eq!(
            execution.detail, None,
            "real results carry no cancel detail"
        );
    }
    let recorded = writer.recorded();
    let synthetic = recorded
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolCallFinished { detail: Some(detail), .. } if detail == "cancelled before the call completed"))
        .count();
    assert_eq!(synthetic, 0, "no synthetic cancel rows for a settled batch");
}

/// §2.5: a panicking tool future fails only its own batch remainder with the
/// JoinError summary in `detail`; the turn continues so the model sees every
/// sibling result, and the panic summary is not a bare static string.
#[tokio::test]
async fn a_panicking_batch_fails_the_rest_without_aborting() {
    struct PanicsThenHangs;
    #[async_trait::async_trait]
    impl ToolRunner for PanicsThenHangs {
        async fn execute(&self, call: &ToolCallRequest) -> Result<ToolOutput> {
            if call.call_id == "boom" {
                panic!("tool exploded: {}", call.call_id);
            }
            std::future::pending::<Result<ToolOutput>>().await
        }
    }
    let writer = Arc::new(MemoryWriter::new());
    let events = crate::exec::TurnEvents::new(ThreadId::new(), TurnId::new());
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(PanicsThenHangs),
        writer: writer.clone(),
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
        ToolCallRequest::new("boom", "read", serde_json::json!({ "path": "a" })),
        ToolCallRequest::new("hang", "read", serde_json::json!({ "path": "b" })),
    ];
    reactor.record_assistant_turn("", &batch);

    let (executions, interrupted) = reactor.run_batch(&batch).await.unwrap();
    // Panic is not a cancel: the turn keeps going with failure rows.
    assert!(!interrupted, "a forwarder panic must not abort the turn");
    assert_eq!(executions.len(), 2);
    let hang = executions
        .iter()
        .find(|execution| execution.call_id == "hang")
        .expect("the hanging call settles synthetically");
    assert!(!hang.ok);
    let detail = hang.detail.as_deref().unwrap_or_default();
    assert!(
        detail.starts_with("tool batch task failed:"),
        "the synthetic row carries the JoinError summary, got: {detail}"
    );
    assert!(
        !detail.ends_with("panicked"),
        "the summary must not be the bare static string"
    );
    let recorded = writer.recorded();
    // join_all drops the whole future tree when one branch panics, including
    // the panicking call's own future: it never produces a channel item, so
    // slot 0 gets the same synthetic failure row as the never-polled sibling.
    let boom_finished = recorded
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::ToolCallFinished {
                call_id, detail, ..
            } if call_id == "boom" => Some(detail.clone()),
            _ => None,
        })
        .expect("the panicking call still needs a paired finished row");
    assert!(
        boom_finished
            .as_deref()
            .unwrap_or_default()
            .starts_with("tool batch task failed:"),
        "its synthetic row carries the panic summary, got: {boom_finished:?}"
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

#[path = "reactor_descendant_tests.rs"]
mod descendant_tests;
