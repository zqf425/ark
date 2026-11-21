//! Reactor unit tests (streaming items, steering rescue, compaction,
//! persistence-before-execute), split out of `reactor.rs` so the production
//! file stays within the AGENTS §4 file-size limit. Declared as a descendant
//! of `reactor.rs`'s `tests` module via `#[path]`, keeping `use super::*`
//! semantics identical to the previous inline mod; the gate and regression
//! suites live in their own files the same way.

use super::*;
use crate::exec::{AllowAllGates, MapToolRunner, MemoryWriter, ToolRunner};
use ark_common::{Result, ThreadId, TurnId};
use ark_contract::RuntimeEvent;

// Gate/append-failure tests live in their own file (AGENTS §4 file-size
// limit); declared as a descendant so `use super::*` reaches the shared
// doubles and the private `ask_approval`/`ask_user`.
#[path = "reactor_gate_tests.rs"]
mod gate_tests;

// Regression tests for the review findings (A1 mixed rounds, A2 ask
// chains, A3 cancelled batches) reuse the doubles through the same
// descendant-module pattern.
#[path = "reactor_regression_tests.rs"]
mod regression_tests;

#[derive(Debug)]
struct NoopModel;

#[async_trait::async_trait]
impl ModelClient for NoopModel {
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
        Ok(ModelResponse::default())
    }
}

#[derive(Debug)]
struct ReadOnlySandbox;

impl SandboxPolicy for ReadOnlySandbox {
    fn decide(&self, _tool: &str, _args: &ToolArgs) -> SandboxDecision {
        SandboxDecision::ReadOnly
    }
}

struct StartedEventRunner {
    writer: Arc<MemoryWriter>,
}

#[async_trait::async_trait]
impl ToolRunner for StartedEventRunner {
    async fn execute(&self, _call: &ToolCallRequest) -> Result<ToolOutput> {
        let started = self
            .writer
            .recorded()
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ToolCallStarted { .. }));
        if started {
            Ok(ToolOutput::ok("executed"))
        } else {
            Ok(ToolOutput::err(
                "tool start was not persisted before execution",
            ))
        }
    }
}

fn reactor(writer: Arc<MemoryWriter>) -> Reactor {
    let events = TurnEvents::new(ThreadId::new(), TurnId::new());
    Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(MapToolRunner::new()),
        writer,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        steer_image_resolver: None,
        steer_outcome_recorder: None,
    })
}

#[tokio::test]
async fn reasoning_and_text_deltas_use_distinct_items() {
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(Arc::clone(&writer));
    let mut acc = Accumulator::default();

    reactor
        .observe(
            ModelChunk::ReasoningDelta {
                delta: "plan".to_string(),
            },
            &mut acc,
        )
        .await
        .unwrap();
    reactor
        .observe(
            ModelChunk::TextDelta {
                delta: "answer".to_string(),
            },
            &mut acc,
        )
        .await
        .unwrap();

    let events = writer.recorded();
    // Each stream now opens with an `ItemCreated`, so select the deltas by
    // kind instead of by position.
    let reasoning_id = events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::AssistantReasoningDelta { item_id, .. } => Some(item_id),
            _ => None,
        })
        .expect("reasoning delta recorded");
    let text_id = events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::AssistantTextDelta { item_id, .. } => Some(item_id),
            _ => None,
        })
        .expect("text delta recorded");
    assert_ne!(reasoning_id, text_id);
}

#[tokio::test]
async fn streamed_items_are_created_and_completed_with_their_final_text() {
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(Arc::clone(&writer));
    let mut acc = Accumulator::default();
    reactor
        .observe(
            ModelChunk::TextDelta {
                delta: "answer".to_string(),
            },
            &mut acc,
        )
        .await
        .unwrap();
    reactor.complete_stream_items().await.unwrap();

    let events = writer.recorded();
    assert!(matches!(events[0], RuntimeEvent::ItemCreated { .. }));
    assert!(matches!(
        &events[2],
        RuntimeEvent::ItemCompleted { item, .. }
            if item.status == ark_contract::ItemStatus::Completed
                && item.text.as_deref() == Some("answer")
    ));
}

#[tokio::test]
async fn approval_promotes_the_call_to_an_executable_decision() {
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    let call = ToolCallRequest::new("call-1", "bash", serde_json::json!({ "cmd": "dir" }))
        .with_sandbox(SandboxDecision::NeedsApproval);

    reactor
        .ask_approval(call, "run dir".to_string())
        .await
        .unwrap();

    assert_eq!(
        reactor.approved[0].sandbox,
        Some(SandboxDecision::WorkspaceWrite)
    );
}

#[tokio::test]
async fn a_denied_decision_never_reaches_the_approval_ask() {
    // A rule-denied call (`SandboxDecision::Deny`) must not open a gate:
    // the refusal is already final, and opening a gate would let an HTTP
    // client approve a call the policy table explicitly rejected.
    let writer = Arc::new(MemoryWriter::new());
    let reactor = reactor(writer);
    let calls = vec![ToolCallRequest::new(
        "call-1",
        "bash",
        serde_json::json!({ "command": "rm -rf /" }),
    )
    .with_sandbox(SandboxDecision::Deny)];

    let asks = reactor.approval_asks(&calls);

    assert!(asks.is_empty(), "a denied call must not open a gate");
    // It rides as a plain call: the runner refuses `Deny` defensively and
    // produces the synthetic failure result, keeping the event pairing.
    assert_eq!(reactor.approval_asks(&calls.clone()).len(), 0);
    let gated: Vec<&ToolCallRequest> = calls.iter().collect();
    assert_eq!(gated.len(), 1);
}

/// Streams an empty response, but during the first (and would-be final)
/// round a steer arrives — the race that used to drop the correction.
#[derive(Debug)]
struct SteerMidStreamModel {
    queue: Arc<SteerQueue>,
    rounds: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl ModelClient for SteerMidStreamModel {
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
        let round = self
            .rounds
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if round == 0 {
            self.queue.push("late correction");
        }
        Ok(ModelResponse::default())
    }
}

#[tokio::test]
async fn a_steer_landing_in_the_final_round_is_not_dropped() {
    let events = TurnEvents::new(ThreadId::new(), TurnId::new());
    let writer = Arc::new(MemoryWriter::for_turn(events.clone()));
    let queue = Arc::new(SteerQueue::new());
    let model = Arc::new(SteerMidStreamModel {
        queue: Arc::clone(&queue),
        rounds: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::clone(&model) as Arc<dyn ModelClient>,
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::clone(&writer) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        steer_image_resolver: None,
        steer_outcome_recorder: None,
    })
    .with_steering(queue);

    let outcome = reactor.run().await.unwrap();

    assert_eq!(outcome, TurnOutcome::Completed);
    // The correction forced a second model round instead of ending the turn.
    assert_eq!(
        model.rounds.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the rescued steer must drive one more round"
    );
    let recorded = writer.recorded();
    let steered = recorded
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::TurnSteered { .. }))
        .count();
    assert_eq!(steered, 1, "the correction is journaled exactly once");
    assert!(
        recorded.iter().any(|event| matches!(
            event,
            RuntimeEvent::ItemCreated { item, .. }
                if item.kind == ark_contract::item::ItemKind::UserMessage
                    && item.text.as_deref() == Some("late correction")
        )),
        "the correction must land in the history as a user item"
    );
    assert!(
        recorded
            .iter()
            .any(|event| matches!(event, RuntimeEvent::TurnCompleted { .. })),
        "the turn still completes"
    );
}

#[tokio::test]
async fn a_cancelled_finish_does_not_rescue_the_steering() {
    let writer = Arc::new(MemoryWriter::new());
    let queue = Arc::new(SteerQueue::new());
    queue.push("late correction");
    let mut reactor = reactor(writer.clone()).with_steering(queue);
    reactor.cancel.cancel();

    let action = reactor
        .rescue_pending_steering(TurnOutcome::Completed)
        .await
        .unwrap();

    assert!(matches!(
        action,
        Action::Finish {
            outcome: TurnOutcome::Completed
        }
    ));
    assert!(!writer
        .recorded()
        .iter()
        .any(|event| matches!(event, RuntimeEvent::TurnSteered { .. })));
}

#[tokio::test]
async fn an_exhausted_budget_does_not_rescue_the_steering() {
    let writer = Arc::new(MemoryWriter::new());
    let queue = Arc::new(SteerQueue::new());
    queue.push("late correction");
    let events = TurnEvents::new(ThreadId::new(), TurnId::new());
    let limits = crate::domain::TurnLimits {
        max_rounds: 0,
        ..crate::domain::TurnLimits::tiny()
    };
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(MapToolRunner::new()),
        writer: Arc::clone(&writer) as Arc<dyn SessionWriter>,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(limits),
        chunk_buffer: 1,
        steer_image_resolver: None,
        steer_outcome_recorder: None,
    })
    .with_steering(queue);

    let action = reactor
        .rescue_pending_steering(TurnOutcome::Completed)
        .await
        .unwrap();

    assert!(matches!(
        action,
        Action::Finish {
            outcome: TurnOutcome::Completed
        }
    ));
    assert!(!writer
        .recorded()
        .iter()
        .any(|event| matches!(event, RuntimeEvent::TurnSteered { .. })));
}

#[tokio::test]
async fn persists_tool_start_before_executing_a_batch() {
    let writer = Arc::new(MemoryWriter::new());
    let events = TurnEvents::new(ThreadId::new(), TurnId::new());
    let mut reactor = Reactor::new(ReactorConfig {
        model: Arc::new(NoopModel),
        tools: Arc::new(StartedEventRunner {
            writer: Arc::clone(&writer),
        }),
        writer,
        gates: Arc::new(AllowAllGates),
        sandbox: Arc::new(ReadOnlySandbox),
        events,
        cancel: CancelToken::new(),
        seed: ModelSeed::default(),
        machine: TurnMachine::new(crate::domain::TurnLimits::tiny()),
        chunk_buffer: 1,
        steer_image_resolver: None,
        steer_outcome_recorder: None,
    });

    let call = ToolCallRequest::new("call-1", "read", serde_json::json!({ "path": "a" }));
    let (executions, interrupted) = reactor.run_batch(&[call]).await.unwrap();

    assert!(!interrupted);
    assert!(executions[0].ok);
}

#[test]
fn compaction_keeps_recent_messages_verbatim() {
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("system prompt"),
        ModelMessage::user("old fact ZEBRA-77"),
        ModelMessage::assistant("old answer"),
        ModelMessage::user("recent user message"),
        ModelMessage::assistant("recent assistant message"),
    ];

    let (removed, _) = reactor.compact_history(2);

    assert_eq!(removed, 2);
    assert_eq!(
        reactor.history[2],
        ModelMessage::user("recent user message")
    );
    assert_eq!(
        reactor.history[3],
        ModelMessage::assistant("recent assistant message")
    );
}

#[test]
fn repeated_compaction_replaces_the_previous_checkpoint() {
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("stable system prompt"),
        ModelMessage::user("Goal: preserve legacy fact ZEBRA-77"),
        ModelMessage::assistant("old answer"),
        ModelMessage::user("first recent message"),
        ModelMessage::assistant("first recent answer"),
    ];

    reactor.compact_history(2);
    reactor
        .history
        .push(ModelMessage::user("second recent message"));
    reactor
        .history
        .push(ModelMessage::assistant("second recent answer"));
    reactor.compact_history(2);

    assert_eq!(
        reactor
            .history
            .iter()
            .filter(|message| message.content.starts_with("[context checkpoint"))
            .count(),
        1
    );
    let checkpoint = reactor
        .history
        .iter()
        .find(|message| message.content.starts_with("[context checkpoint"))
        .expect("a generated checkpoint remains");
    assert!(checkpoint.content.contains("ZEBRA-77"));
    assert!(checkpoint
        .content
        .contains("Goal: preserve legacy fact ZEBRA-77"));
    // The durable boundary must summarise the kept tail too: the next turn
    // replays only this note plus rows written after it.
    assert!(checkpoint.content.contains("first recent message"));
    assert_eq!(
        reactor.history[0],
        ModelMessage::system("stable system prompt")
    );
    assert_eq!(
        reactor.history[2],
        ModelMessage::user("second recent message")
    );
    assert_eq!(
        reactor.history[3],
        ModelMessage::assistant("second recent answer")
    );
}

#[test]
fn compaction_restates_the_plan_at_the_history_tail() {
    // A mid-turn compaction drops the turn-seed plan line (it sits at the
    // seed tail, after the replayed history rows, so it is foldable) into
    // the checkpoint note at the fold boundary, ahead of the kept tail;
    // later rounds push it ever further from the recency window
    // (lost-in-the-middle). The plan must also be restated as the LAST
    // history row, in-memory only, so the model keeps seeing it at the near
    // position for the rest of the turn.
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("stable system prompt"),
        ModelMessage::system("workspace root: D:/repo"),
        ModelMessage::user("old user message"),
        ModelMessage::assistant("old answer"),
        ModelMessage::system(
            "[thread plan - current objective and progress]\n\
             Goal: repair the parser\n\
             - [~] fix wire format\n\
             - [ ] run tests",
        ),
        ModelMessage::user("recent user message"),
        ModelMessage::assistant("recent assistant message"),
    ];

    let (removed, _) = reactor.compact_history(2);

    assert_eq!(removed, 3);
    let last = reactor.history.last().expect("history keeps rows");
    assert_eq!(last.role, ModelRole::System);
    assert!(last.content.contains("Goal: repair the parser"));
    assert!(last.content.contains("- [~] fix wire format"));
    assert!(last.content.contains("- [ ] run tests"));
    assert!(
        HistoryHygiene::new().is_legal(&reactor.history),
        "restated plan must keep history provider-legal: {:?}",
        reactor.history
    );
}

#[test]
fn compaction_skips_the_plan_restate_without_plan_facts() {
    // No Goal/checkbox rows in the folded region: nothing to restate, and an
    // empty tail note would be pure noise before the next user message.
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("stable system prompt"),
        ModelMessage::user("old user message QUARTZ-11"),
        ModelMessage::assistant("old answer"),
        ModelMessage::user("recent user message"),
        ModelMessage::assistant("recent assistant message"),
    ];

    let (removed, _) = reactor.compact_history(2);

    assert_eq!(removed, 2);
    assert_eq!(
        reactor.history.last().map(|message| message.role),
        Some(ModelRole::Assistant),
    );
}

#[test]
fn repeated_compaction_keeps_exactly_one_plan_restate() {
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("stable system prompt"),
        ModelMessage::system("workspace root: D:/repo"),
        ModelMessage::user("first old message"),
        ModelMessage::assistant("first old answer"),
        ModelMessage::system(
            "[thread plan - current objective and progress]\nGoal: ship it\n- [ ] step one",
        ),
        ModelMessage::user("first recent message"),
        ModelMessage::assistant("first recent answer"),
    ];
    reactor.compact_history(2);
    reactor
        .history
        .push(ModelMessage::user("second recent message"));
    reactor
        .history
        .push(ModelMessage::assistant("second recent answer"));

    reactor.compact_history(2);

    let plan_restates = reactor
        .history
        .iter()
        .filter(|message| message.content.contains("Goal: ship it"))
        .count();
    assert_eq!(
        plan_restates, 2,
        "one occurrence in the checkpoint note + one tail restate: {:?}",
        reactor.history
    );
    let tail_restates = reactor
        .history
        .iter()
        .filter(|message| message.content.starts_with("[thread plan]\n"))
        .count();
    assert_eq!(
        tail_restates, 1,
        "repeated compactions must not stack tail restates: {:?}",
        reactor.history
    );
    assert_eq!(
        reactor.history.last().map(|message| message.role),
        Some(ModelRole::System),
    );
}

#[test]
fn repeated_compaction_within_keep_recent_keeps_one_plan_restate() {
    // N1b: the restate row sits at the tail, so a second compaction whose
    // `keep_recent` window still covers it would preserve the old row AND
    // push a new one — two `[thread plan]` rows separated by kept messages,
    // which hygiene cannot collapse (it only folds adjacent duplicates).
    // The rebuild must drop any surviving restate rows before pushing the
    // fresh one.
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("stable system prompt"),
        ModelMessage::system("workspace root: D:/repo"),
        ModelMessage::user("first old message"),
        ModelMessage::assistant("first old answer"),
        ModelMessage::system(
            "[thread plan - current objective and progress]\nGoal: ship it\n- [ ] step one",
        ),
        ModelMessage::user("first recent message"),
        ModelMessage::assistant("first recent answer"),
    ];
    reactor.compact_history(2);
    // One large tool output already crosses the threshold again; the tail
    // (old restate + 2 rows) is smaller than keep_recent, so the old restate
    // survives inside the kept window.
    reactor
        .history
        .push(ModelMessage::user("second recent message"));
    reactor
        .history
        .push(ModelMessage::assistant("second recent answer"));

    reactor.compact_history(4);

    let tail_restates = reactor
        .history
        .iter()
        .filter(|message| message.content.starts_with("[thread plan]\n"))
        .count();
    assert_eq!(
        tail_restates, 1,
        "a restate kept by keep_recent must be replaced, not duplicated: {:?}",
        reactor.history
    );
    assert_eq!(
        reactor.history.last().map(|message| message.role),
        Some(ModelRole::System),
    );
}

/// A `tool` role row answering `call_id`, the shape the provider requires.
fn tool_message(call_id: &str, name: &str, output: &str) -> ModelMessage {
    ModelMessage {
        role: ModelRole::Tool,
        content: output.to_string(),
        images: Vec::new(),
        tool_call_id: Some(call_id.to_string()),
        name: Some(name.to_string()),
        tool_calls: Vec::new(),
    }
}

#[test]
fn compaction_never_starts_the_kept_tail_with_a_tool_result() {
    // A kept tail that opens with a tool result has no declaring assistant
    // row, and every provider rejects that request. The split walks back to
    // the assistant row instead of trusting `keep_recent` blindly.
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("system prompt"),
        ModelMessage::user("old question"),
        ModelMessage::assistant("old answer"),
        ModelMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRef {
                call_id: "c1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({ "path": "a" }),
            }],
        ),
        tool_message("c1", "read", "file body"),
        ModelMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRef {
                call_id: "c2".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({ "path": "b" }),
            }],
        ),
        tool_message("c2", "read", "file body two"),
    ];

    // keep=1 would start the tail at the `c2` result.
    let (removed, _) = reactor.compact_history(1);

    assert_eq!(removed, 4);
    assert_eq!(reactor.history[2].role, ModelRole::Assistant);
    assert_eq!(reactor.history[2].tool_calls[0].call_id, "c2");
    assert_eq!(reactor.history[3].role, ModelRole::Tool);
    assert!(
        HistoryHygiene::new().is_legal(&reactor.history),
        "compacted history must stay provider-legal: {:?}",
        reactor.history
    );
    // Both results are still represented, through the checkpoint note.
    assert!(reactor
        .history
        .iter()
        .any(|message| message.content.starts_with("[context checkpoint")));
}

#[test]
fn compaction_reports_a_noop_when_the_prefix_is_one_call_run() {
    // Nothing can be folded without splitting a call/result pair, so the fold
    // must report zero removed items instead of persisting an empty boundary.
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("system prompt"),
        ModelMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRef {
                call_id: "c1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({ "path": "a" }),
            }],
        ),
        tool_message("c1", "read", "file body"),
    ];
    let before = reactor.history.clone();

    let (removed, checkpoint) = reactor.compact_history(1);

    assert_eq!(removed, 0);
    assert!(checkpoint.is_none());
    assert_eq!(reactor.history, before);
}

#[tokio::test]
async fn a_productive_fold_does_not_spend_the_retry_budget() {
    // The retry budget bounds a fold that cannot shrink the prefix, not the
    // number of folds: a long turn may legitimately fold once per
    // `context_limit_tokens` of new tool output.
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("system prompt"),
        ModelMessage::user("a".repeat(2_000)),
        ModelMessage::assistant("old answer"),
        ModelMessage::user("recent user message"),
    ];

    reactor.compact(2).await.unwrap();

    assert_eq!(
        reactor.machine().progress().compactions,
        0,
        "a fold that bought headroom must not consume the retry budget"
    );
}

#[tokio::test]
async fn a_fold_that_cannot_shrink_the_prefix_spends_the_retry_budget() {
    // Otherwise an oversized but incompressible prefix would compact forever:
    // nothing is foldable without splitting a call/result pair, and the kept
    // tail alone is already past `context_limit_tokens`.
    let writer = Arc::new(MemoryWriter::new());
    let mut reactor = reactor(writer);
    reactor.history = vec![
        ModelMessage::system("system prompt"),
        ModelMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRef {
                call_id: "c1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({ "path": "a" }),
            }],
        ),
        tool_message("c1", "read", &"a".repeat(20_000)),
    ];

    reactor.compact(1).await.unwrap();

    assert_eq!(reactor.machine().progress().compactions, 1);
}

#[test]
fn assistant_tool_call_history_is_legal_hygiene() {
    let mut reactor = reactor(Arc::new(MemoryWriter::new()));
    let calls = vec![ToolCallRequest::new(
        "c1",
        "read",
        serde_json::json!({"path": "a.txt"}),
    )];
    reactor.record_assistant_turn("calling tool", &calls);
    reactor.record_assistant_turn("done", &[]);

    let history = reactor.history();
    assert_eq!(history[0].role, ModelRole::Assistant);
    assert_eq!(history[0].content, "calling tool");
    assert_eq!(history[0].tool_calls.len(), 1);
    assert_eq!(history[0].tool_calls[0].call_id, "c1");
    assert_eq!(history[0].tool_calls[0].name, "read");
    assert!(
        HistoryHygiene::new().is_legal(history),
        "recorded assistant turn must satisfy history hygiene"
    );
}
