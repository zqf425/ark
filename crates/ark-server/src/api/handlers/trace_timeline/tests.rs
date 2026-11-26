//! Timeline projection tests, mounted from `handlers/trace_timeline.rs`.

use super::*;
use ark_common::Usage;

fn at(offset_ms: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::milliseconds(offset_ms)
}

fn started(seq: u64, thread: ThreadId, turn: TurnId, at_ms: i64) -> RuntimeEvent {
    RuntimeEvent::TurnStarted {
        auto_recovery: 0,
        seq,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(at_ms),
        auto_continuation: false,
        attachment_ids: Vec::new(),
        attachment_outcomes: Vec::new(),
    }
}

fn completed(seq: u64, thread: ThreadId, turn: TurnId, at_ms: i64) -> RuntimeEvent {
    RuntimeEvent::TurnCompleted {
        seq,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(at_ms),
    }
}

/// An item event; `kind` decides the span, `seq` only orders the log and
/// seeds a deterministic item id (the same keyed-id scheme the store uses
/// for events without an explicit id).
fn item_event(
    seq: u64,
    thread: ThreadId,
    turn: TurnId,
    at_ms: i64,
    kind: ItemKind,
    text: &str,
) -> RuntimeEvent {
    let mut item = ThreadItem::new(turn, kind);
    item.item_id = ark_common::keyed_item_id(&format!("test_item:{seq}"));
    item.created_at = at(at_ms);
    item.text = (!text.is_empty()).then(|| text.to_string());
    RuntimeEvent::ItemCreated {
        seq,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(at_ms),
        item,
    }
}

fn tool_started(
    seq: u64,
    thread: ThreadId,
    turn: TurnId,
    at_ms: i64,
    call: &str,
    tool: &str,
) -> RuntimeEvent {
    RuntimeEvent::ToolCallStarted {
        seq,
        thread_id: thread,
        turn_id: turn,
        item_id: Some(tool_item_id(call)),
        timestamp: at(at_ms),
        call_id: call.to_string(),
        tool: tool.to_string(),
        arguments: serde_json::json!({}),
    }
}

#[allow(clippy::too_many_arguments)] // mirrors the wire event's field list
fn tool_finished(
    seq: u64,
    thread: ThreadId,
    turn: TurnId,
    at_ms: i64,
    call: &str,
    tool: &str,
    ok: bool,
    duration_ms: u64,
    output: &str,
) -> RuntimeEvent {
    RuntimeEvent::ToolCallFinished {
        seq,
        thread_id: thread,
        turn_id: turn,
        item_id: Some(tool_item_id(call)),
        timestamp: at(at_ms),
        call_id: call.to_string(),
        tool: tool.to_string(),
        ok,
        duration_ms,
        output: output.to_string(),
        detail: None,
    }
}

/// The `ToolCall` item the store would project for one finished call.
fn tool_item(turn: TurnId, at_ms: i64, call: &str, tool: &str, ok: bool) -> ThreadItem {
    let mut item = ThreadItem::new(turn, ItemKind::ToolCall);
    item.item_id = tool_item_id(call);
    item.created_at = at(at_ms);
    item.tool = Some(ToolPayload {
        call_id: call.to_string(),
        tool: tool.to_string(),
        arguments: None,
        output: Some("output body".to_string()),
        ok: Some(ok),
        duration_ms: Some(300),
    });
    item
}

fn usage_at(
    seq: u64,
    thread: ThreadId,
    turn: TurnId,
    at_ms: i64,
    duration_ms: Option<u64>,
) -> RuntimeEvent {
    let mut usage = Usage::new(100, 40);
    usage.duration_ms = duration_ms;
    RuntimeEvent::Usage {
        seq,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(at_ms),
        usage,
    }
}

fn error_at(seq: u64, thread: ThreadId, at_ms: i64, message: &str) -> RuntimeEvent {
    RuntimeEvent::Error {
        seq,
        thread_id: thread,
        timestamp: at(at_ms),
        code: ark_common::ArkErrorCode::InternalError,
        message: message.to_string(),
    }
}

fn kinds(timeline: &TraceTimeline) -> Vec<TraceSpanKind> {
    timeline.spans.iter().map(|span| span.kind).collect()
}

#[test]
fn unknown_turn_is_not_found() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    // No `turn_started` for `turn` anywhere in the log: the caller maps
    // the `None` this returns onto the structured 404.
    assert!(build_trace_timeline(thread, &[], turn).is_none());
    let other = TurnId::new();
    let events = vec![started(1, thread, other, 0)];
    assert!(build_trace_timeline(thread, &events, turn).is_none());
}

#[test]
fn empty_turn_yields_an_empty_span_list() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![started(1, thread, turn, 0), completed(2, thread, turn, 50)];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert!(timeline.spans.is_empty());
    assert_eq!(timeline.turn_id, turn);
    assert_eq!(timeline.last_seq, Some(2));
}

#[test]
fn a_normal_turn_sorts_all_kinds_by_start_time() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let mut user = ThreadItem::new(turn, ItemKind::UserMessage);
    user.item_id = ark_common::keyed_item_id("test_user");
    user.created_at = at(10);
    user.text = Some("帮我看看".to_string());
    user.model = Some("openai:gpt-5".to_string());
    let events = vec![
        started(1, thread, turn, 0),
        RuntimeEvent::ItemCreated {
            seq: 2,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(10),
            item: user,
        },
        usage_at(3, thread, turn, 20, Some(5)),
        // Model call: ends at 20ms, lasts 5ms → starts at 15ms, after
        // the user message at 10ms and before the tool at 30ms.
        tool_started(4, thread, turn, 30, "c1", "read"),
        RuntimeEvent::ItemCompleted {
            seq: 5,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(60),
            item: tool_item(turn, 30, "c1", "read", true),
        },
        completed(6, thread, turn, 500),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(
        kinds(&timeline),
        vec![
            TraceSpanKind::UserMessage,
            TraceSpanKind::ModelCall,
            TraceSpanKind::ToolCall,
        ]
    );
    // The model span starts at `end - duration`, not at the end instant.
    let model = &timeline.spans[1];
    assert_eq!(model.started_at, at(15));
    // The tool span starts at `ToolCallStarted` time, not item creation.
    let tool = &timeline.spans[2];
    assert_eq!(tool.started_at, at(30));
    assert_eq!(tool.duration_ms, Some(300));
    assert_eq!(tool.label.as_deref(), Some("read"));
    assert!(!tool.is_error);
    let model = &timeline.spans[1];
    assert_eq!(model.duration_ms, Some(5));
    assert_eq!(model.label.as_deref(), Some("openai:gpt-5"));
    let user_span = &timeline.spans[0];
    assert_eq!(user_span.duration_ms, None);
    assert_eq!(user_span.text_snippet.as_deref(), Some("帮我看看"));
}

#[test]
fn tool_spans_come_from_events_without_item_events() {
    // Real-log shape: `ToolCall` items are a read-side store projection,
    // the event log itself only carries started/finished events. The
    // timeline must still show the tool call.
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0),
        tool_started(2, thread, turn, 10, "c1", "bash"),
        tool_finished(3, thread, turn, 40, "c1", "bash", true, 300, "ok"),
        completed(4, thread, turn, 100),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(kinds(&timeline), vec![TraceSpanKind::ToolCall]);
    let span = &timeline.spans[0];
    // Start instant is the authoritative `ToolCallStarted` time.
    assert_eq!(span.started_at, at(10));
    assert_eq!(span.duration_ms, Some(300));
    assert_eq!(span.label.as_deref(), Some("bash"));
}

#[test]
fn item_backed_tool_rows_do_not_duplicate_event_rows() {
    // A log that somehow carries both the events and the projected item
    // must not render the call twice: the event row wins.
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0),
        tool_started(2, thread, turn, 10, "c1", "bash"),
        tool_finished(3, thread, turn, 40, "c1", "bash", true, 300, "ok"),
        RuntimeEvent::ItemCompleted {
            seq: 4,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(40),
            item: tool_item(turn, 10, "c1", "bash", true),
        },
        completed(5, thread, turn, 100),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(kinds(&timeline), vec![TraceSpanKind::ToolCall]);
}

#[test]
fn an_unfinished_tool_call_survives_as_a_point() {
    // Started but never finished (crash/cancel): no event row exists, and
    // the projected item carries no duration (the store only writes one
    // on finish) -- the leftover keeps the activity visible as a point.
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let mut leftover = tool_item(turn, 10, "c1", "bash", true);
    if let Some(payload) = leftover.tool.as_mut() {
        payload.duration_ms = None;
    }
    let events = vec![
        started(1, thread, turn, 0),
        tool_started(2, thread, turn, 10, "c1", "bash"),
        RuntimeEvent::ItemCompleted {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(40),
            item: leftover,
        },
        completed(4, thread, turn, 100),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(kinds(&timeline), vec![TraceSpanKind::ToolCall]);
    assert_eq!(timeline.spans[0].duration_ms, None);
}

#[test]
fn a_failed_tool_call_is_flagged() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0),
        tool_started(2, thread, turn, 10, "c1", "bash"),
        RuntimeEvent::ItemCompleted {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(40),
            item: tool_item(turn, 10, "c1", "bash", false),
        },
        completed(4, thread, turn, 100),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(timeline.spans.len(), 1);
    let span = &timeline.spans[0];
    assert_eq!(span.kind, TraceSpanKind::ToolCall);
    assert!(span.is_error);
    assert_eq!(span.duration_ms, Some(300));
}

#[test]
fn text_items_are_not_rows_and_streamed_versions_collapse() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let mut item = ThreadItem::new(turn, ItemKind::AssistantMessage);
    item.item_id = ark_common::keyed_item_id("test_assistant");
    item.created_at = at(10);
    let created = RuntimeEvent::ItemCreated {
        seq: 2,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(10),
        item: item.clone(),
    };
    item.text = Some("partial".to_string());
    let updated = RuntimeEvent::ItemUpdated {
        seq: 3,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(20),
        item: item.clone(),
    };
    item.text = Some("full reply text".to_string());
    let finished = RuntimeEvent::ItemCompleted {
        seq: 4,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(30),
        item,
    };
    let events = vec![
        started(1, thread, turn, 0),
        created,
        updated,
        finished,
        // A measured window owns the text at 10ms: [0, 20].
        usage_at(5, thread, turn, 20, Some(20)),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    // No independent assistant/reasoning row: the Gantt axis only shows
    // the model call that streamed the text.
    assert_eq!(kinds(&timeline), vec![TraceSpanKind::ModelCall]);
    let details = timeline.spans[0].details.as_ref().expect("details");
    assert_eq!(details.len(), 1);
    // The last version wins, matching the thread-detail projection.
    assert_eq!(details[0].text_snippet, "full reply text");
    assert_eq!(details[0].started_at, at(10));
    assert_eq!(details[0].kind, TraceSpanDetailKind::AssistantText);
}

#[test]
fn text_fragments_go_to_the_narrowest_containing_model_call() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0),
        // Outer window [0, 100]; nested window [40, 60].
        usage_at(2, thread, turn, 100, Some(100)),
        item_event(
            3,
            thread,
            turn,
            20,
            ItemKind::AssistantMessage,
            "outer reply",
        ),
        usage_at(4, thread, turn, 60, Some(20)),
        item_event(5, thread, turn, 55, ItemKind::Reasoning, "later thought"),
        item_event(9, thread, turn, 50, ItemKind::Reasoning, "inner thought"),
        // Another window, with text at its right boundary.
        usage_at(6, thread, turn, 200, Some(20)),
        item_event(7, thread, turn, 200, ItemKind::Reasoning, "edge thought"),
        completed(8, thread, turn, 300),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(
        kinds(&timeline),
        vec![
            TraceSpanKind::ModelCall,
            TraceSpanKind::ModelCall,
            TraceSpanKind::ModelCall,
        ]
    );
    // The outer window only owns text the nested window does not contain.
    let details = timeline.spans[0].details.as_ref().expect("outer details");
    assert_eq!(details.len(), 1);
    assert_eq!(details[0].text_snippet, "outer reply");
    // The 50ms fragment goes to the 20ms window, not the 100ms one.
    let details = timeline.spans[1].details.as_ref().expect("nested details");
    assert_eq!(details.len(), 2);
    // Same kind, several fragments: all kept and sorted by `started_at`.
    assert_eq!(details[0].text_snippet, "inner thought");
    assert_eq!(details[0].kind, TraceSpanDetailKind::Reasoning);
    assert_eq!(details[0].started_at, at(50));
    assert_eq!(details[1].text_snippet, "later thought");
    assert_eq!(details[1].started_at, at(55));
    // Windows are closed: text at the end instant belongs to them.
    let details = timeline.spans[2].details.as_ref().expect("last details");
    assert_eq!(details.len(), 1);
    assert_eq!(details[0].text_snippet, "edge thought");
    assert_eq!(details[0].started_at, at(200));
}

#[test]
fn text_outside_every_window_or_without_any_window_is_dropped() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0),
        // Measured window [40, 50]: text at 10ms and 60ms is outside.
        usage_at(2, thread, turn, 50, Some(10)),
        item_event(3, thread, turn, 10, ItemKind::AssistantMessage, "too early"),
        item_event(4, thread, turn, 60, ItemKind::Reasoning, "too late"),
        // Old log: no duration means no window, so its text has no owner.
        usage_at(5, thread, turn, 100, None),
        item_event(
            6,
            thread,
            turn,
            100,
            ItemKind::AssistantMessage,
            "no window",
        ),
        completed(7, thread, turn, 200),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(
        kinds(&timeline),
        vec![TraceSpanKind::ModelCall, TraceSpanKind::ModelCall,]
    );
    assert!(timeline.spans.iter().all(|span| span.details.is_none()));
}

#[test]
fn model_call_without_text_has_no_details() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0),
        usage_at(2, thread, turn, 90, Some(80)),
        completed(3, thread, turn, 100),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(timeline.spans.len(), 1);
    assert!(timeline.spans[0].details.is_none());
}

#[test]
fn snippets_are_cut_on_character_boundaries() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let long = "汉".repeat(TRACE_TIMELINE_SNIPPET_MAX_CHARS + 50);
    let events = vec![
        started(1, thread, turn, 0),
        item_event(2, thread, turn, 10, ItemKind::Notice, &long),
        completed(3, thread, turn, 50),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    let snippet = timeline.spans[0].text_snippet.as_deref().expect("snippet");
    assert_eq!(snippet.chars().count(), TRACE_TIMELINE_SNIPPET_MAX_CHARS);
    // Every cut code point is the full character, never a broken byte.
    assert!(snippet.chars().all(|c| c == '汉'));
}

#[test]
fn usage_events_become_model_call_spans() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0),
        // Measured round: ends at 90ms, lasts 80ms → starts at 10ms.
        usage_at(2, thread, turn, 90, Some(80)),
        // Unmeasured round: starts (and stays) at its end instant --
        // never a invented start or a fake zero duration.
        usage_at(3, thread, turn, 200, None),
        completed(4, thread, turn, 300),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(timeline.spans.len(), 2);
    assert_eq!(timeline.spans[0].kind, TraceSpanKind::ModelCall);
    assert_eq!(timeline.spans[0].duration_ms, Some(80));
    assert_eq!(timeline.spans[1].duration_ms, None);
    // The usage timestamp is the round's END: the measured span starts at
    // `end - duration` (90-80=10ms), so consecutive rounds tile the axis
    // instead of collapsing to points (the "gantt has holes" defect).
    assert_eq!(timeline.spans[0].started_at, at(10));
    assert_eq!(timeline.spans[1].started_at, at(200));
}

#[test]
fn compaction_produces_one_spanning_row() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let mut checkpoint = ThreadItem::new(turn, ItemKind::Notice);
    checkpoint.item_id = ark_common::keyed_item_id("test_checkpoint");
    checkpoint.created_at = at(60);
    checkpoint.text = Some("[context checkpoint] folded".to_string());
    checkpoint.marker = Some(ark_contract::ItemMarker::ContextCheckpoint);
    let events = vec![
        started(1, thread, turn, 0),
        RuntimeEvent::CompactionStarted {
            seq: 2,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(20),
        },
        RuntimeEvent::ItemCreated {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(60),
            item: checkpoint,
        },
        RuntimeEvent::CompactionCompleted {
            seq: 4,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(120),
            removed_items: 5,
            checkpoint_chars: 28,
        },
        completed(5, thread, turn, 200),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(timeline.spans.len(), 1);
    let span = &timeline.spans[0];
    assert_eq!(span.kind, TraceSpanKind::Compaction);
    assert_eq!(span.started_at, at(20));
    assert_eq!(span.duration_ms, Some(100));
}

#[test]
fn raw_error_events_become_error_spans_of_the_open_turn() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let other = TurnId::new();
    // Two turns in one thread: only the open turn owns each error.
    let events = vec![
        started(1, thread, other, 0),
        completed(2, thread, other, 5),
        started(3, thread, turn, 10),
        error_at(4, thread, 20, "boom"),
        // After the terminal event the error belongs to no turn.
        completed(5, thread, turn, 30),
        error_at(6, thread, 40, "outside"),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(timeline.spans.len(), 1);
    let span = &timeline.spans[0];
    assert_eq!(span.kind, TraceSpanKind::Error);
    assert!(span.is_error);
    assert_eq!(span.text_snippet.as_deref(), Some("boom"));
    assert_eq!(span.item_id, None);
}

#[test]
fn the_other_turns_rows_do_not_leak_into_this_timeline() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let other = TurnId::new();
    let events = vec![
        started(1, thread, other, 0),
        item_event(2, thread, other, 10, ItemKind::Notice, "other turn"),
        usage_at(3, thread, other, 20, Some(5)),
        started(4, thread, turn, 30),
        item_event(5, thread, turn, 40, ItemKind::Notice, "this turn"),
        completed(6, thread, turn, 50),
    ];
    let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
    assert_eq!(timeline.spans.len(), 1);
    assert_eq!(timeline.spans[0].text_snippet.as_deref(), Some("this turn"));
}
