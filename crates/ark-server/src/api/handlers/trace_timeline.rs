//! `GET /v1/threads/{id}/trace-timeline?turn_id=…`
//! (requirements/trace-analysis.md §v2).
//!
//! Like the trace-stats / usage projections, the whole readout is a pure fold
//! of one thread's authoritative event log: nothing is persisted, no second
//! copy of any item or usage state, and one replay belongs to this handler
//! alone (no shared cache with trace-stats -- both stay stateless read-side
//! projections). The fold reuses the thread-detail item collapse rule
//! (`ItemCreated/Updated/Completed` keep the last version per item id) so the
//! timeline can never disagree with the conversation it drills into.

use std::collections::HashMap;

use ark_common::{ItemId, ThreadId, TurnId};
use ark_contract::item::ToolPayload;
use ark_contract::{
    ItemKind, RuntimeEvent, ThreadItem, TraceSpan, TraceSpanDetail, TraceSpanDetailKind,
    TraceSpanKind, TraceTimeline, TRACE_TIMELINE_SNIPPET_MAX_CHARS,
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

/// A measured model-call window: text is attributed to the narrowest window
/// containing its creation instant. Unmeasured calls have no window.
#[derive(Debug, Clone, Copy)]
struct ModelWindow {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    duration_ms: u64,
}

/// The turn-scoped state one fold pass accumulates before spans are cut.
#[derive(Default)]
struct TimelineFold {
    /// Last version per item id of this turn's items, first-seen order --
    /// the same collapse thread detail applies to streamed items.
    items: HashMap<ItemId, ThreadItem>,
    order: Vec<ItemId>,
    /// Tool calls started in this turn, keyed by call id. The real log has
    /// no `ToolCall` item events -- the `ToolCall` item only exists as the
    /// store's read-side projection (`ark-store/items.rs` applies the raw
    /// events) -- so tool spans must come from the events themselves:
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
    /// One row per `usage` event of this turn, in log order. Measured rounds
    /// supply the windows that own streamed text fragments; the windows are
    /// kept separately so attribution can pick the narrowest one.
    model_calls: Vec<TraceSpan>,
    model_windows: Vec<ModelWindow>,
    /// Streamed reasoning / assistant text items of this turn, keyed by item
    /// id so streamed versions collapse like thread detail does. They never
    /// become rows; [`attach_text_details`] folds them into model-call rows.
    text_details: HashMap<ItemId, TraceSpanDetail>,
    /// The turn currently open, per the turn lifecycle events.
    open_turn: Option<TurnId>,
    turn_seen: bool,
}

/// Project one turn's timeline from the thread's event log.
///
/// `None` when the log never started the turn -- the caller turns that into a
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
    attach_text_details(
        &mut spans,
        &fold.model_windows,
        fold.text_details.values().cloned().collect(),
    );
    // Raw `RuntimeEvent::Error` rows carry no turn id, so the fold already
    // attributed them to the turn that was open when they were logged -- the
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
            // missing (pre-field logs), the span stays a point at the end --
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
                if let Some(duration_ms) = usage.duration_ms {
                    // Only a measured call yields a window. Older logs with
                    // no duration cannot own text: there is no honest start
                    // to test `created_at` against, so those fragments are
                    // dropped from this projection (see the contract docs).
                    let end = *timestamp;
                    let start = end
                        - chrono::Duration::milliseconds(
                            i64::try_from(duration_ms).unwrap_or(i64::MAX),
                        );
                    self.model_windows.push(ModelWindow {
                        start,
                        end,
                        duration_ms,
                    });
                }
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
            details: None,
        });
    }

    /// Keep the newest version per item id, remembering first-seen order.
    /// Text items stay out of the row map: they are folded into model-call
    /// details after the whole log has been seen (the narrowest window may
    /// only become known on a later `usage` event).
    fn observe_item(&mut self, item: &ThreadItem) {
        let kind = match item.kind {
            ItemKind::Reasoning => Some(TraceSpanDetailKind::Reasoning),
            ItemKind::AssistantMessage => Some(TraceSpanDetailKind::AssistantText),
            _ => None,
        };
        if let Some(kind) = kind {
            if let Some(text) = item.text.as_deref().filter(|text| !text.is_empty()) {
                self.text_details.insert(
                    item.item_id,
                    TraceSpanDetail {
                        kind,
                        text_snippet: cut_snippet(text),
                        started_at: item.created_at,
                    },
                );
            }
            return;
        }
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
        // Tool spans come from `ToolCallFinished` events -- the only source on
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
/// item creation when the start event never landed in the log). Streamed
/// reasoning / assistant text returns `None` -- it is child content of a model
/// call, attached to that row instead of taking Gantt space.
fn item_span(item: &ThreadItem, open_calls: &HashMap<String, DateTime<Utc>>) -> Option<TraceSpan> {
    // A context checkpoint is a compaction boundary, not a plain notice.
    // (When the compaction event pair exists, the caller keeps one row per
    // compaction by skipping the item copy -- the events carry the window.)
    if item.is_context_checkpoint() {
        return Some(plain_span(TraceSpanKind::Compaction, item));
    }
    let kind = match item.kind {
        ItemKind::UserMessage => TraceSpanKind::UserMessage,
        ItemKind::AssistantMessage | ItemKind::Reasoning => return None,
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
        details: None,
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
        details: None,
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
        details: None,
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
        details: None,
    }
}

/// Attach streamed text fragments to the narrowest model-call window that
/// contains each fragment's `created_at`.
///
/// A text item is a child event of the model round streaming it, never a row
/// on the Gantt axis. Multiple fragments in one window are all kept, sorted by
/// `started_at`. Fragments outside every window are dropped: older logs have
/// no `usage.duration_ms`, hence no window to own them, and inventing one
/// would place text on a timeline it never occupied. The full text remains
/// readable on thread detail.
fn attach_text_details(
    spans: &mut [TraceSpan],
    windows: &[ModelWindow],
    details: Vec<TraceSpanDetail>,
) {
    for (index, window) in windows.iter().enumerate() {
        let mut owned: Vec<TraceSpanDetail> = details
            .iter()
            .filter(|detail| {
                detail.started_at >= window.start
                    && detail.started_at <= window.end
                    && narrowest_owner(detail.started_at, windows) == Some(index)
            })
            .cloned()
            .collect();
        if owned.is_empty() {
            continue;
        }
        // Stable sort: fragments sharing an instant keep log insertion order.
        owned.sort_by_key(|detail| detail.started_at);
        // Windows are pushed in the same order as `model_calls` spans; the
        // model rows are appended in that order and only sorted afterwards,
        // so we match by the window's exact start/duration instead of assuming
        // an index survives the sort.
        if let Some(span) = spans.iter_mut().find(|span| {
            span.kind == TraceSpanKind::ModelCall
                && span.started_at == window.start
                && span.duration_ms == Some(window.duration_ms)
        }) {
            span.details = Some(owned);
        }
    }
}

/// Index of the shortest measured window containing `at`, if any.
fn narrowest_owner(at: DateTime<Utc>, windows: &[ModelWindow]) -> Option<usize> {
    windows
        .iter()
        .enumerate()
        .filter(|(_, window)| at >= window.start && at <= window.end)
        .min_by_key(|(_, window)| window.duration_ms)
        .map(|(index, _)| index)
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
#[path = "trace_timeline/tests.rs"]
mod tests;
