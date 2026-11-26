//! `GET /v1/threads/{id}/trace-timeline?turn_id=…`
//! (requirements/trace-analysis.md §v2).
//!
//! Like the trace-stats / usage projections, the whole readout is a pure fold
//! of one thread's authoritative event log: nothing is persisted, no second
//! copy of any item or usage state, and one replay belongs to this handler
//! alone (no shared cache with trace-stats — both stay stateless read-side
//! projections). The fold reuses the thread-detail item collapse rule
//! (`ItemCreated/Updated/Completed` keep the last version per item id) so the
//! timeline can never disagree with the conversation it drills into.

use std::collections::HashMap;

use ark_common::{ItemId, ThreadId, TurnId};
use ark_contract::item::ToolPayload;
use ark_contract::{
    ItemKind, RuntimeEvent, ThreadItem, TraceSpan, TraceSpanKind, TraceTimeline,
    TRACE_TIMELINE_SNIPPET_MAX_CHARS,
};
use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct TraceTimelineQuery {
    /// The single turn the timeline drills into; required.
    #[serde(default)]
    pub turn_id: Option<String>,
}

/// `GET /v1/threads/{id}/trace-timeline?turn_id=…`.
///
/// Read-only projection with the same boundaries as trace-stats: the whole
/// log of one thread, no gate interaction. A missing `turn_id` is a
/// structured 400; a turn the log never started is the same structured 404
/// an unknown thread gets, so a stale chart row cannot keep an empty panel
/// forever.
pub async fn trace_timeline(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TraceTimelineQuery>,
) -> Result<Json<TraceTimeline>, ApiError> {
    let thread = parse_thread(&id)?;
    let turn = parse_turn(query.turn_id.as_deref())?;
    state.store.get_thread(thread).await?;
    let events = state.store.replay_events(thread, 0).await?;
    build_trace_timeline(thread, &events, turn)
        .ok_or_else(|| turn_not_found(&turn))
        .map(Json)
}

/// Parse the query turn id, reporting a validation error instead of a 500.
fn parse_turn(raw: Option<&str>) -> Result<TurnId, ApiError> {
    let Some(raw) = raw else {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            "turn_id query parameter is required",
        ));
    };
    raw.parse::<TurnId>().map_err(|err| {
        ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            format!("`{raw}` is not a valid turn id: {err}"),
        )
    })
}

fn turn_not_found(turn: &TurnId) -> ApiError {
    ApiError::new(
        ark_common::ArkErrorCode::NotFound,
        format!("turn `{turn}` was not started in this thread"),
    )
}

/// The turn-scoped state one fold pass accumulates before spans are cut.
#[derive(Default)]
struct TimelineFold {
    /// Last version per item id of this turn's items, first-seen order —
    /// the same collapse thread detail applies to streamed items.
    items: HashMap<ItemId, ThreadItem>,
    order: Vec<ItemId>,
    /// Tool calls started in this turn, keyed by call id. The real log has
    /// no `ToolCall` item events — the `ToolCall` item only exists as the
    /// store's read-side projection (`ark-store/items.rs` applies the raw
    /// events) — so tool spans must come from the events themselves:
    /// `ToolCallStarted` supplies the true start instant, `ToolCallFinished`
    /// the payload. Item-backed tool rows are then deduped against these.
    open_calls: HashMap<String, DateTime<Utc>>,
    /// One row per `ToolCallFinished` of this turn, keyed by call id; the
    /// authoritative source of tool spans on real logs.
    tool_calls: HashMap<String, TraceSpan>,
    /// `provider:model` label the turn's opening user item carries; labels a
    /// `model_call` row when the `usage` event itself carries no name.
    model_label: Option<String>,
    /// Compaction observed for this turn: start timestamp plus optional end.
    compaction: Option<(DateTime<Utc>, Option<DateTime<Utc>>)>,
    /// Rows cut from raw `RuntimeEvent::Error` records attributed to this
    /// turn: the event itself carries no turn id, so the turn that was open
    /// when the error was logged owns it (llm-rounds / trace-stats rule).
    error_spans: Vec<TraceSpan>,
    /// One row per `usage` event of this turn, in log order.
    model_calls: Vec<TraceSpan>,
    /// The turn currently open, per the turn lifecycle events.
    open_turn: Option<TurnId>,
    turn_seen: bool,
}

/// Project one turn's timeline from the thread's event log.
///
/// `None` when the log never started the turn — the caller turns that into a
/// structured 404. Tool calls, compactions and error events come straight
/// from events; item-backed kinds collapse `ItemCreated` / `ItemUpdated` /
/// `ItemCompleted` per item id (last version wins), which is the store's own
/// projection rule and the reason a streamed item surfaces exactly once
/// however many versions it streamed through.
pub fn build_trace_timeline(
    thread_id: ThreadId,
    events: &[RuntimeEvent],
    turn_id: TurnId,
) -> Option<TraceTimeline> {
    let mut fold = TimelineFold::default();
    for event in events {
        fold.observe(event, turn_id);
    }
    if !fold.turn_seen {
        return None;
    }
    let mut spans = fold.spans();
    // Raw `RuntimeEvent::Error` rows carry no turn id, so the fold already
    // attributed them to the turn that was open when they were logged — the
    // same rule the llm-rounds and trace-stats projections apply.
    let error_spans = fold.take_error_spans();
    spans.extend(error_spans);
    // Stable sort keeps the log order for rows that share a timestamp.
    spans.sort_by_key(|span| span.started_at);
    let last_seq = events.last().map(RuntimeEvent::seq);
    Some(TraceTimeline {
        thread_id,
        turn_id,
        spans,
        last_seq,
    })
}

impl TimelineFold {
    /// Watch every event, but only fold the ones this turn owns.
    fn observe(&mut self, event: &RuntimeEvent, turn: TurnId) {
        if let RuntimeEvent::TurnStarted { turn_id, .. } = event {
            if *turn_id == turn {
                self.turn_seen = true;
            }
        }
        self.observe_open_turn(event);
        match event {
            RuntimeEvent::ItemCreated { turn_id, item, .. }
            | RuntimeEvent::ItemUpdated { turn_id, item, .. }
            | RuntimeEvent::ItemCompleted { turn_id, item, .. } => {
                if *turn_id == turn {
                    self.observe_item(item);
                }
            }
            RuntimeEvent::ToolCallStarted {
                turn_id,
                call_id,
                timestamp,
                ..
            } => {
                if *turn_id == turn {
                    self.open_calls.insert(call_id.clone(), *timestamp);
                }
            }
            RuntimeEvent::ToolCallFinished {
                turn_id,
                timestamp,
                call_id,
                tool,
                ok,
                duration_ms,
                output,
                detail,
                ..
            } if *turn_id == turn => {
                // The start instant is the authoritative `ToolCallStarted`
                // time; fall back to `end - duration` so a missing start
                // event still yields a correctly sized bar, and only a log
                // missing both degrades to a point at the end.
                let started = self.open_calls.get(call_id).copied().unwrap_or_else(|| {
                    *timestamp
                        - chrono::Duration::milliseconds(
                            i64::try_from(*duration_ms).unwrap_or(i64::MAX),
                        )
                });
                let span = tool_span(
                    started,
                    &ToolPayload {
                        call_id: call_id.clone(),
                        tool: tool.clone(),
                        arguments: None,
                        output: (!output.is_empty()).then(|| output.clone()),
                        ok: Some(*ok),
                        duration_ms: Some(*duration_ms),
                    },
                    None,
                );
                self.tool_calls.insert(call_id.clone(), span);
            }
            RuntimeEvent::CompactionStarted {
                turn_id, timestamp, ..
            } => {
                if *turn_id == turn {
                    self.compaction = Some((*timestamp, None));
                }
            }
            RuntimeEvent::CompactionCompleted {
                turn_id, timestamp, ..
            } => {
                if *turn_id == turn {
                    self.observe_compaction_completed(*timestamp);
                }
            }
            RuntimeEvent::Error {
                timestamp, message, ..
            } => self.observe_error(*timestamp, message, turn),
            // One usage event = one completed model round; the agent loop
            // measured its wall clock. The usage timestamp is the round's
            // END: the span starts at `end - duration` so model rounds tile
            // the timeline instead of collapsing to points at their end
            // instants (the "gantt has holes" defect). When the duration is
            // missing (pre-field logs), the span stays a point at the end —
            // no invented start.
            RuntimeEvent::Usage {
                turn_id,
                timestamp,
                usage,
                ..
            } if *turn_id == turn => {
                let started = match usage.duration_ms {
                    Some(ms) => {
                        *timestamp
                            - chrono::Duration::milliseconds(i64::try_from(ms).unwrap_or(i64::MAX))
                    }
                    None => *timestamp,
                };
                self.model_calls.push(model_call_span(
                    started,
                    usage.duration_ms,
                    self.model_label.clone(),
                ));
            }
            _ => {}
        }
        // The model label only rides the opening user item; reading it from
        // the item (not thread metadata) keeps a mid-session switch from
        // mislabeling this turn's model calls.
        if let RuntimeEvent::ItemCreated { turn_id, item, .. } = event {
            if *turn_id == turn && item.kind == ItemKind::UserMessage {
                self.model_label = item.model.clone();
            }
        }
    }

    /// Track which turn is open so turn-less events (raw errors) can be
    /// attributed the way the other projections do. Terminal events close
    /// the turn: a later error belongs to no drill-down row.
    fn observe_open_turn(&mut self, event: &RuntimeEvent) {
        self.open_turn = match event {
            RuntimeEvent::TurnStarted { turn_id, .. } => Some(*turn_id),
            RuntimeEvent::TurnCompleted { .. }
            | RuntimeEvent::TurnFailed { .. }
            | RuntimeEvent::TurnAborted { .. } => None,
            _ => self.open_turn,
        };
    }

    /// Raw errors carry no turn id: the turn that was open when the error was
    /// logged owns the row, and errors outside any turn belong to no
    /// drill-down row at all.
    fn observe_error(&mut self, timestamp: DateTime<Utc>, message: &str, turn: TurnId) {
        if self.open_turn != Some(turn) {
            return;
        }
        self.error_spans.push(TraceSpan {
            kind: TraceSpanKind::Error,
            started_at: timestamp,
            duration_ms: None,
            label: None,
            text_snippet: Some(cut_snippet(message)),
            is_error: true,
            item_id: None,
        });
    }

    /// Keep the newest version per item id, remembering first-seen order.
    fn observe_item(&mut self, item: &ThreadItem) {
        if !self.items.contains_key(&item.item_id) {
            self.order.push(item.item_id);
        }
        self.items.insert(item.item_id, item.clone());
    }

    /// A completed compaction without a paired start still shows the
    /// boundary happened in this turn; with one, it closes the window.
    fn observe_compaction_completed(&mut self, at: DateTime<Utc>) {
        match &mut self.compaction {
            Some((_, end @ None)) => *end = Some(at),
            _ => self.compaction = Some((at, Some(at))),
        }
    }

    /// Cut the timeline rows from the folded state.
    fn spans(&mut self) -> Vec<TraceSpan> {
        let mut spans: Vec<TraceSpan> = Vec::new();
        for id in &self.order {
            let Some(item) = self.items.get(id) else {
                continue;
            };
            // One compaction is recorded twice (checkpoint item + the
            // start/completed event pair). The event pair carries the real
            // window, so when it exists the item row would only duplicate it.
            if item.is_context_checkpoint() && self.compaction.is_some() {
                continue;
            }
            // Tool calls never take the item path: real logs carry no
            // `ToolCall` item events, so the only legitimate item rows here
            // are unfinished-call leftovers, handled below from the item map.
            if item.kind == ItemKind::ToolCall {
                continue;
            }
            if let Some(span) = item_span(item, &self.open_calls) {
                spans.push(span);
            }
        }
        if let Some((started, ended)) = self.compaction {
            spans.push(compaction_span(started, ended));
        }
        // Tool spans come from `ToolCallFinished` events — the only source on
        // real logs (`ToolCall` items are a read-side store projection of the
        // same events, which is why the item path above skips them).
        let mut event_tools: Vec<TraceSpan> = self.tool_calls.values().cloned().collect();
        // A call the log started but never finished (crash, cancel) has no
        // event row: keep its item-derived point instead of dropping the
        // activity entirely.
        for item in self.items.values() {
            let Some(payload) = item.tool.as_ref() else {
                continue;
            };
            if self.tool_calls.contains_key(&payload.call_id) {
                continue;
            }
            if let Some(span) = item_span(item, &self.open_calls) {
                event_tools.push(span);
            }
        }
        spans.append(&mut event_tools);
        spans.append(&mut self.model_calls);
        // Stable sort keeps the log order for rows that share a timestamp.
        spans.sort_by_key(|span| span.started_at);
        spans
    }

    /// Hand over the raw-error rows collected for this turn.
    fn take_error_spans(&mut self) -> Vec<TraceSpan> {
        std::mem::take(&mut self.error_spans)
    }
}

/// The item id a tool-call event writes, matching the store's fallback key.
///
/// Test-only today: real logs carry an explicit `item_id` on tool events, but
/// the deterministic `tool_call:{call_id}` key is what `ark-store` projects
/// for events without one, so the tests use it to build matching items.
#[cfg(test)]
fn tool_item_id(call_id: &str) -> ItemId {
    ark_common::keyed_item_id(&format!("tool_call:{call_id}"))
}

/// Map a collapsed item to its timeline row; items without a row fall out.
///
/// `open_calls` supplies the `ToolCallStarted` timestamp (falling back to
/// item creation when the start event never landed in the log).
fn item_span(item: &ThreadItem, open_calls: &HashMap<String, DateTime<Utc>>) -> Option<TraceSpan> {
    // A context checkpoint is a compaction boundary, not a plain notice.
    // (When the compaction event pair exists, the caller keeps one row per
    // compaction by skipping the item copy — the events carry the window.)
    if item.is_context_checkpoint() {
        return Some(plain_span(TraceSpanKind::Compaction, item));
    }
    let kind = match item.kind {
        ItemKind::UserMessage => TraceSpanKind::UserMessage,
        ItemKind::AssistantMessage => TraceSpanKind::AssistantText,
        ItemKind::Reasoning => TraceSpanKind::Reasoning,
        ItemKind::ToolCall => return tool_span_from_item(item, open_calls),
        ItemKind::Approval => TraceSpanKind::Approval,
        ItemKind::UserInput => TraceSpanKind::UserInput,
        ItemKind::Error => return Some(error_span(item)),
        ItemKind::Notice => TraceSpanKind::Notice,
        // Nothing in the runtime produces `tool_result` rows; the kind only
        // exists for wire compatibility with the item vocabulary.
        ItemKind::ToolResult => return None,
    };
    Some(plain_span(kind, item))
}

/// An item-backed row: creation timestamp, no measured duration.
fn plain_span(kind: TraceSpanKind, item: &ThreadItem) -> TraceSpan {
    TraceSpan {
        kind,
        started_at: item.created_at,
        duration_ms: None,
        label: None,
        text_snippet: item.text.as_deref().map(cut_snippet),
        is_error: false,
        item_id: Some(item.item_id),
    }
}

/// An error row is always flagged, whatever wrote the item.
fn error_span(item: &ThreadItem) -> TraceSpan {
    let mut span = plain_span(TraceSpanKind::Error, item);
    span.is_error = true;
    span
}

/// The tool-call row from a collapsed `ToolCall` item.
fn tool_span_from_item(
    item: &ThreadItem,
    open_calls: &HashMap<String, DateTime<Utc>>,
) -> Option<TraceSpan> {
    let payload = item.tool.as_ref()?;
    let started_at = open_calls
        .get(&payload.call_id)
        .copied()
        .unwrap_or(item.created_at);
    Some(tool_span(started_at, payload, Some(item.item_id)))
}

/// One tool-call row: measured duration from the payload, error flag from
/// `ok`. Only a bounded output prefix rides the row; the full text stays on
/// thread detail.
fn tool_span(
    started_at: DateTime<Utc>,
    payload: &ToolPayload,
    item_id: Option<ItemId>,
) -> TraceSpan {
    // Output first; a call without output falls back to a stringified
    // arguments prefix so the row still identifies the call. Both are capped
    // the same way, and only the bounded prefix rides the wire.
    let summary = payload
        .output
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map_or_else(
            || {
                payload
                    .arguments
                    .as_ref()
                    .filter(|value| !value.is_null())
                    .map(|value| value.to_string())
            },
            |text| Some(text.to_string()),
        );
    TraceSpan {
        kind: TraceSpanKind::ToolCall,
        started_at,
        duration_ms: payload.duration_ms,
        label: Some(payload.tool.clone()),
        text_snippet: summary.as_deref().map(cut_snippet),
        is_error: payload.ok == Some(false),
        item_id,
    }
}

/// One compaction row spanning `CompactionStarted` → `CompactionCompleted`.
fn compaction_span(started_at: DateTime<Utc>, ended_at: Option<DateTime<Utc>>) -> TraceSpan {
    TraceSpan {
        kind: TraceSpanKind::Compaction,
        started_at,
        duration_ms: ended_at.map(|end| (end - started_at).num_milliseconds().max(0) as u64),
        label: None,
        text_snippet: None,
        is_error: false,
        item_id: None,
    }
}

/// One `model_call` row per `usage` event.
///
/// Honest approximation: `LlmSpanStart`/`LlmSpanEnd` have no producer yet, so
/// the duration is the wall clock the agent loop recorded when it closed the
/// round, and the label falls back to the `provider:model` stamp the turn's
/// opening user item carries (absent on older logs).
fn model_call_span(
    started_at: DateTime<Utc>,
    duration_ms: Option<u64>,
    label: Option<String>,
) -> TraceSpan {
    TraceSpan {
        kind: TraceSpanKind::ModelCall,
        started_at,
        duration_ms,
        label,
        text_snippet: None,
        is_error: false,
        item_id: None,
    }
}

/// Cap a summary to `TRACE_TIMELINE_SNIPPET_MAX_CHARS` characters.
///
/// Character-based on purpose: the wire summary must not cut a multi-byte
/// code point in half, and the byte-level `truncate_utf8` would silently
/// shorten CJK text fourfold. `chars().take` never splits a code point.
fn cut_snippet(text: &str) -> String {
    text.chars()
        .take(TRACE_TIMELINE_SNIPPET_MAX_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
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
            // the user message at 10s and before the tool at 30s.
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
        // on finish) — the leftover keeps the activity visible as a point.
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
    fn streamed_item_versions_collapse_into_one_span() {
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
        let events = vec![started(1, thread, turn, 0), created, updated, finished];
        let timeline = build_trace_timeline(thread, &events, turn).expect("turn seen");
        assert_eq!(timeline.spans.len(), 1);
        // The last version wins, matching the thread-detail projection.
        assert_eq!(
            timeline.spans[0].text_snippet.as_deref(),
            Some("full reply text")
        );
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
            // Unmeasured round: starts (and stays) at its end instant —
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
}
