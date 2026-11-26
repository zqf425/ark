//! Descendant regressions (AG-03 onward), split from the parent suite to
//! honor the 1000-line limit while retaining access to its private doubles.

use super::*;

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
        ToolCallRequest::new("c3", "read", serde_json::json!({ "path": "c" })),
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
    // The second Started append fails. Only slot zero has a durable start, so
    // compensation must not manufacture Finished rows for c2 or c3.
    assert_eq!(
        finished,
        [(
            "c1",
            Some("tool event persistence failed before completion")
        )],
        "only durable starts are compensated: {recorded:?}"
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
