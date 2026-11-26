//! `GET /v1/threads/{id}/trace-stats` (requirements/trace-analysis.md).
//!
//! Like the usage / llm-rounds projections, the whole readout is a pure fold
//! of one thread's authoritative event log: nothing is persisted and there is
//! no second copy of any usage, tool or error state. The first version
//! projects the existing event vocabulary only — `LlmSpanStart`/`LlmSpanEnd`
//! stay planned, so "model call" means one `usage` event and "tool call"
//! means a `tool_call_finished` row.

use std::collections::{HashMap, HashSet};

use ark_common::{ThreadId, TurnId, Usage};
use ark_contract::{
    RuntimeEvent, ToolStat, TraceErrorEntry, TraceRuntimeError, TraceSlowCall, TraceStatsResponse,
    TurnStat, TRACE_SLOW_CALLS_LIMIT,
};
use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, NaiveDate, Utc};

use crate::api::error::ApiError;
use crate::api::handlers::diagnostics::ActiveClock;
use crate::api::handlers::threads::parse_thread;
use crate::state::AppState;

/// `GET /v1/threads/{id}/trace-stats`.
///
/// Read-only projection with the same boundaries as the usage route: the whole
/// log of one thread, no parameters, no gate interaction.
pub async fn trace_stats(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TraceStatsResponse>, ApiError> {
    let thread = parse_thread(&id)?;
    state.store.get_thread(thread).await?;
    let events = state.store.replay_events(thread, 0).await?;
    Ok(Json(aggregate_trace_stats(thread, &events)))
}

/// Bounded summary length for tool arguments on a slow-call row.
///
/// The full arguments already live on the event log; the wire row only needs
/// enough context to identify the call in the chart.
const ARGUMENT_SUMMARY_MAX_BYTES: usize = 160;

/// A finished tool call, before it lands in the per-tool / slow-call folds.
struct ToolObservation {
    seq: u64,
    turn_id: TurnId,
    call_id: String,
    tool: String,
    finished_at: DateTime<Utc>,
    ok: bool,
    duration_ms: u64,
    /// Short one-line prefix of the call's arguments for the slow-call list.
    arguments_summary: String,
}

/// Per-turn accumulator, keyed in first-seen (log) order by the caller.
struct TurnAccum {
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    model_rounds: u64,
    tool_calls: u64,
    failed_tool_calls: u64,
    usage: Usage,
    usage_samples: u64,
    /// Failed row currently owning turn-less runtime errors. Every finished
    /// tool call clears it; only a failed finish opens a new window.
    error_sink: Option<usize>,
    failed_rows: Vec<TraceErrorEntry>,
}

/// Single-pass fold over one thread's event log.
#[derive(Default)]
struct TraceFold {
    order: Vec<TurnId>,
    seen: HashSet<TurnId>,
    per_turn: HashMap<TurnId, TurnAccum>,
    /// In-flight calls keyed by `(turn_id, call_id)`; the runtime pairs a
    /// finish with its start by call id inside one turn, so a call that never
    /// finished simply leaves no observation behind.
    open_calls: HashMap<(TurnId, String), String>,
    finished: Vec<ToolObservation>,
    /// The turn runtime errors currently belong to.
    current_turn: Option<TurnId>,
    usage: Usage,
    model_calls: u64,
    days: HashSet<NaiveDate>,
}

/// Compact one-line prefix of a call's JSON arguments.
///
/// `serde_json::to_string` is already single-line, so only the byte cap
/// applies; a null payload folds to an empty summary.
fn summarize_arguments(arguments: &serde_json::Value) -> String {
    if arguments.is_null() {
        return String::new();
    }
    ark_common::truncate_utf8(&arguments.to_string(), ARGUMENT_SUMMARY_MAX_BYTES).to_string()
}

/// Project trace statistics for one thread from its event log.
///
/// One pass builds the turn rows, tool distribution, slow calls and error
/// windows from the same iteration, so the session totals can never disagree
/// with the per-turn breakdown. Turns and tool aggregates keep first-seen
/// order; only the slow-call list is deliberately re-ordered.
pub fn aggregate_trace_stats(thread_id: ThreadId, events: &[RuntimeEvent]) -> TraceStatsResponse {
    let mut fold = TraceFold::default();
    for event in events {
        fold.observe(event);
    }
    fold.finish(thread_id, events)
}

impl TraceFold {
    fn observe(&mut self, event: &RuntimeEvent) {
        self.days.insert(event.timestamp().date_naive());
        match event {
            RuntimeEvent::TurnStarted { turn_id, .. } => self.observe_turn_started(*turn_id, event),
            RuntimeEvent::Usage { turn_id, usage, .. } => self.observe_usage(*turn_id, usage),
            RuntimeEvent::ToolCallStarted {
                turn_id,
                call_id,
                arguments,
                ..
            } => {
                self.open_calls
                    .insert((*turn_id, call_id.clone()), summarize_arguments(arguments));
            }
            RuntimeEvent::ToolCallFinished {
                seq,
                turn_id,
                call_id,
                tool,
                ok,
                duration_ms,
                timestamp,
                output,
                detail,
                ..
            } => self.observe_tool_finished(
                *seq,
                *turn_id,
                call_id,
                tool,
                *ok,
                *duration_ms,
                *timestamp,
                output,
                detail.as_deref(),
            ),
            RuntimeEvent::Error {
                seq,
                timestamp,
                code,
                message,
                ..
            } => self.observe_error(*seq, *timestamp, *code, message),
            RuntimeEvent::TurnCompleted { turn_id, .. }
            | RuntimeEvent::TurnFailed { turn_id, .. }
            | RuntimeEvent::TurnAborted { turn_id, .. } => {
                self.observe_turn_ended(*turn_id, event.timestamp())
            }
            _ => {}
        }
    }

    /// Open a turn: first-seen registration and a fresh accumulator.
    fn observe_turn_started(&mut self, turn_id: TurnId, event: &RuntimeEvent) {
        if self.seen.insert(turn_id) {
            self.order.push(turn_id);
        }
        self.current_turn = Some(turn_id);
        self.per_turn.entry(turn_id).or_insert_with(|| TurnAccum {
            started_at: event.timestamp(),
            ended_at: None,
            model_rounds: 0,
            tool_calls: 0,
            failed_tool_calls: 0,
            usage: Usage::default(),
            usage_samples: 0,
            error_sink: None,
            failed_rows: Vec::new(),
        });
    }

    fn observe_usage(&mut self, turn_id: TurnId, usage: &Usage) {
        self.model_calls += 1;
        self.usage.merge(usage);
        if let Some(acc) = self.per_turn.get_mut(&turn_id) {
            acc.model_rounds += 1;
            acc.usage_samples += 1;
            acc.usage.merge(usage);
        }
    }

    /// Close one tool call: the distribution observation, plus a failed-call
    /// error row when the turn is already tracked.
    #[allow(clippy::too_many_arguments)] // mirrors the wire event's field list
    fn observe_tool_finished(
        &mut self,
        seq: u64,
        turn_id: TurnId,
        call_id: &str,
        tool: &str,
        ok: bool,
        duration_ms: u64,
        finished_at: DateTime<Utc>,
        output: &str,
        detail: Option<&str>,
    ) {
        let arguments_summary = self
            .open_calls
            .remove(&(turn_id, call_id.to_string()))
            .unwrap_or_default();
        if let Some(acc) = self.per_turn.get_mut(&turn_id) {
            // A finish closes the previous failure's error window even when
            // this call succeeds; a later Error must not attach backwards.
            acc.error_sink = None;
            acc.tool_calls += 1;
            if !ok {
                acc.failed_tool_calls += 1;
                acc.failed_rows.push(TraceErrorEntry {
                    seq,
                    turn_id,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    timestamp: finished_at,
                    duration_ms,
                    // `detail`, else the capped output: the failure reason
                    // the error list shows for this row.
                    detail: detail
                        .map(str::to_string)
                        .or_else(|| (!output.is_empty()).then(|| output.to_string())),
                    errors: Vec::new(),
                });
                acc.error_sink = Some(acc.failed_rows.len() - 1);
            }
        }
        self.finished.push(ToolObservation {
            seq,
            turn_id,
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            finished_at,
            ok,
            duration_ms,
            arguments_summary,
        });
    }

    /// Append a runtime error only to the failed call whose finish window is
    /// still open. Errors before the first failure or after a successful finish
    /// have no failed row to own them.
    fn observe_error(
        &mut self,
        seq: u64,
        timestamp: DateTime<Utc>,
        code: ark_common::ArkErrorCode,
        message: &str,
    ) {
        let Some(turn_id) = self.current_turn else {
            return;
        };
        let Some(acc) = self.per_turn.get_mut(&turn_id) else {
            return;
        };
        let Some(sink) = acc.error_sink else {
            return;
        };
        if let Some(row) = acc.failed_rows.get_mut(sink) {
            row.errors.push(TraceRuntimeError {
                seq,
                timestamp,
                code,
                message: message.to_string(),
            });
        }
    }

    fn observe_turn_ended(&mut self, turn_id: TurnId, ended_at: DateTime<Utc>) {
        if let Some(acc) = self.per_turn.get_mut(&turn_id) {
            acc.ended_at = Some(ended_at);
            acc.error_sink = None;
        }
        if self.current_turn == Some(turn_id) {
            self.current_turn = None;
        }
    }

    /// Assemble the response: per-turn rows, distributions and the clock fold.
    fn finish(mut self, thread_id: ThreadId, events: &[RuntimeEvent]) -> TraceStatsResponse {
        // The gate-exclusion fold in `diagnostics` is the single host of the
        // turn wall-clock semantics; reuse it instead of re-timing per turn.
        let mut clock = ActiveClock::default();
        for event in events {
            clock.observe(event);
        }
        let spans = clock.finish();
        let mut failed_rows = Vec::new();
        let turn_stats: Vec<TurnStat> = self
            .order
            .iter()
            .map(|turn_id| {
                let acc = self.per_turn.remove(turn_id).expect("turn seen in fold");
                failed_rows.extend(acc.failed_rows.iter().cloned());
                let span = spans.turns.get(turn_id);
                TurnStat {
                    turn_id: *turn_id,
                    started_at: acc.started_at,
                    ended_at: acc.ended_at,
                    active_ms: span.map_or(0, |span| span.active_ms()),
                    active: acc.ended_at.is_none(),
                    model_rounds: acc.model_rounds,
                    tool_calls: acc.tool_calls,
                    failed_tool_calls: acc.failed_tool_calls,
                    usage: (acc.usage_samples > 0).then(|| acc.usage.clone()),
                }
            })
            .collect();
        let turns = turn_stats.len() as u64;
        TraceStatsResponse {
            thread_id,
            turns,
            total_duration_ms: thread_span(events),
            active_ms: (turns > 0).then_some(spans.total_ms),
            model_calls: self.model_calls,
            usage: (self.model_calls > 0).then(|| self.usage.clone()),
            tool_calls: self.finished.len() as u64,
            failed_tool_calls: failed_rows.len() as u64,
            active_days: self.days.len() as u64,
            last_event_at: events.last().map(RuntimeEvent::timestamp),
            turn_stats,
            tool_stats: tool_distribution(&self.finished),
            slowest_calls: slowest_calls(&self.finished),
            errors: failed_rows,
            last_seq: events.last().map(RuntimeEvent::seq),
        }
    }
}

/// Wall-clock span anchored by turn lifecycle events.
///
/// The first `TurnStarted` opens the thread span and the current turn's
/// terminal closes it. Non-turn events never move a settled endpoint; only a
/// final still-running turn extends to the newest event in the log.
fn thread_span(events: &[RuntimeEvent]) -> Option<u64> {
    let mut first = None;
    let mut last = None;
    let mut open_turn = None;
    for event in events {
        match event {
            RuntimeEvent::TurnStarted {
                turn_id, timestamp, ..
            } => {
                first.get_or_insert(*timestamp);
                last = Some(*timestamp);
                open_turn = Some(*turn_id);
            }
            RuntimeEvent::TurnCompleted {
                turn_id, timestamp, ..
            }
            | RuntimeEvent::TurnFailed {
                turn_id, timestamp, ..
            }
            | RuntimeEvent::TurnAborted {
                turn_id, timestamp, ..
            } if open_turn == Some(*turn_id) => {
                last = Some(*timestamp);
                open_turn = None;
            }
            _ => {}
        }
    }
    if open_turn.is_some() {
        last = events.last().map(RuntimeEvent::timestamp);
    }
    Some((last? - first?).num_milliseconds().max(0) as u64)
}

/// Fold the finished calls into per-tool rows ordered by call count.
///
/// Ties break on the tool name so the chart does not reshuffle between
/// reloads of the same log. `duration_ms` is written for every finish, but a
/// zero total keeps `None` rather than reporting a fake zero average.
fn tool_distribution(finished: &[ToolObservation]) -> Vec<ToolStat> {
    struct ToolAccum {
        calls: u64,
        failures: u64,
        total_duration_ms: u64,
    }
    let mut per_tool: HashMap<&str, ToolAccum> = HashMap::new();
    for call in finished {
        let acc = per_tool.entry(call.tool.as_str()).or_insert(ToolAccum {
            calls: 0,
            failures: 0,
            total_duration_ms: 0,
        });
        acc.calls += 1;
        if !call.ok {
            acc.failures += 1;
        }
        acc.total_duration_ms += call.duration_ms;
    }
    let mut rows: Vec<ToolStat> = per_tool
        .into_iter()
        .map(|(tool, acc)| ToolStat {
            tool: tool.to_string(),
            calls: acc.calls,
            failures: acc.failures,
            avg_duration_ms: (acc.calls > 0).then(|| acc.total_duration_ms / acc.calls),
        })
        .collect();
    rows.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.tool.cmp(&b.tool)));
    rows
}

/// The slowest finished calls, `duration_ms` descending, capped.
///
/// Ties keep log order (timestamp, then seq), so re-running the projection
/// over an unchanged log returns the same rows in the same order.
fn slowest_calls(finished: &[ToolObservation]) -> Vec<TraceSlowCall> {
    let mut ranked: Vec<&ToolObservation> = finished.iter().collect();
    ranked.sort_by(|a, b| {
        b.duration_ms
            .cmp(&a.duration_ms)
            .then_with(|| a.finished_at.cmp(&b.finished_at))
            .then_with(|| a.seq.cmp(&b.seq))
    });
    ranked
        .into_iter()
        .take(TRACE_SLOW_CALLS_LIMIT)
        .map(|call| TraceSlowCall {
            seq: call.seq,
            turn_id: call.turn_id,
            call_id: call.call_id.clone(),
            tool: call.tool.clone(),
            timestamp: call.finished_at,
            duration_ms: call.duration_ms,
            arguments_summary: call.arguments_summary.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::ArkErrorCode;

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

    fn usage_at(seq: u64, thread: ThreadId, turn: TurnId, at_ms: i64, prompt: u64) -> RuntimeEvent {
        RuntimeEvent::Usage {
            seq,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(at_ms),
            usage: Usage::new(prompt, prompt / 2),
        }
    }

    fn tool_started(
        seq: u64,
        thread: ThreadId,
        turn: TurnId,
        at_ms: i64,
        call: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> RuntimeEvent {
        RuntimeEvent::ToolCallStarted {
            seq,
            thread_id: thread,
            turn_id: turn,
            item_id: None,
            timestamp: at(at_ms),
            call_id: call.to_string(),
            tool: tool.to_string(),
            arguments,
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
        tool_finished_full(
            seq,
            thread,
            turn,
            at_ms,
            call,
            tool,
            ok,
            duration_ms,
            output,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // mirrors the wire event's field list
    fn tool_finished_full(
        seq: u64,
        thread: ThreadId,
        turn: TurnId,
        at_ms: i64,
        call: &str,
        tool: &str,
        ok: bool,
        duration_ms: u64,
        output: &str,
        detail: Option<&str>,
    ) -> RuntimeEvent {
        RuntimeEvent::ToolCallFinished {
            seq,
            thread_id: thread,
            turn_id: turn,
            item_id: None,
            timestamp: at(at_ms),
            call_id: call.to_string(),
            tool: tool.to_string(),
            ok,
            duration_ms,
            output: output.to_string(),
            detail: detail.map(str::to_string),
        }
    }

    fn error_at(seq: u64, thread: ThreadId, at_ms: i64, message: &str) -> RuntimeEvent {
        RuntimeEvent::Error {
            seq,
            thread_id: thread,
            timestamp: at(at_ms),
            code: ArkErrorCode::InternalError,
            message: message.to_string(),
        }
    }

    #[test]
    fn empty_log_reports_zeroed_response() {
        let thread = ThreadId::new();
        let response = aggregate_trace_stats(thread, &[]);
        assert_eq!(response.thread_id, thread);
        assert_eq!(response.turns, 0);
        assert_eq!(response.total_duration_ms, None);
        assert_eq!(response.active_ms, None);
        assert_eq!(response.model_calls, 0);
        assert_eq!(response.usage, None);
        assert_eq!(response.tool_calls, 0);
        assert_eq!(response.failed_tool_calls, 0);
        assert_eq!(response.active_days, 0);
        assert_eq!(response.last_event_at, None);
        assert!(response.turn_stats.is_empty());
        assert!(response.tool_stats.is_empty());
        assert!(response.slowest_calls.is_empty());
        assert!(response.errors.is_empty());
        assert_eq!(response.last_seq, None);
    }

    #[test]
    fn mixed_session_folds_turns_usage_and_tools() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0),
            usage_at(2, thread, turn, 100, 100),
            tool_started(
                3,
                thread,
                turn,
                150,
                "c1",
                "read",
                serde_json::json!({"path": "a.rs"}),
            ),
            tool_finished(4, thread, turn, 450, "c1", "read", true, 300, ""),
            usage_at(5, thread, turn, 500, 50),
            completed(6, thread, turn, 1_000),
        ];
        let response = aggregate_trace_stats(thread, &events);
        assert_eq!(response.turns, 1);
        assert_eq!(response.model_calls, 2);
        assert_eq!(response.usage.as_ref().unwrap().prompt_tokens, 150);
        assert_eq!(response.tool_calls, 1);
        assert_eq!(response.failed_tool_calls, 0);
        assert_eq!(response.active_days, 1);
        assert_eq!(response.total_duration_ms, Some(1_000));
        // Gate-excluded clock: same fold as the usage route.
        assert_eq!(response.active_ms, Some(1_000));
        assert_eq!(response.turn_stats.len(), 1);
        let row = &response.turn_stats[0];
        assert_eq!(row.turn_id, turn);
        assert_eq!(row.active_ms, 1_000);
        assert!(!row.active);
        assert_eq!(row.model_rounds, 2);
        assert_eq!(row.tool_calls, 1);
        assert_eq!(row.failed_tool_calls, 0);
        assert_eq!(row.usage.as_ref().unwrap().prompt_tokens, 150);
        assert_eq!(response.last_seq, Some(6));
        assert_eq!(response.tool_stats.len(), 1);
        assert_eq!(response.tool_stats[0].tool, "read");
        assert_eq!(response.tool_stats[0].calls, 1);
        assert_eq!(response.tool_stats[0].avg_duration_ms, Some(300));
        assert_eq!(response.slowest_calls.len(), 1);
        assert_eq!(response.slowest_calls[0].duration_ms, 300);
        assert!(response.slowest_calls[0].arguments_summary.contains("a.rs"));
        assert!(response.errors.is_empty());
    }

    #[test]
    fn failed_tool_call_lands_in_the_error_list() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0),
            tool_started(
                2,
                thread,
                turn,
                100,
                "c1",
                "bash",
                serde_json::json!({"cmd": "x"}),
            ),
            tool_finished(3, thread, turn, 400, "c1", "bash", false, 300, "boom"),
            completed(4, thread, turn, 500),
        ];
        let response = aggregate_trace_stats(thread, &events);
        assert_eq!(response.failed_tool_calls, 1);
        assert_eq!(response.errors.len(), 1);
        let entry = &response.errors[0];
        assert_eq!(entry.call_id, "c1");
        assert_eq!(entry.tool, "bash");
        assert_eq!(entry.duration_ms, 300);
        assert_eq!(entry.detail.as_deref(), Some("boom"));
        // No Error events: the row still exists but carries none.
        assert!(entry.errors.is_empty());
        let row = &response.turn_stats[0];
        assert_eq!(row.failed_tool_calls, 1);
        assert_eq!(response.tool_stats[0].failures, 1);
    }

    #[test]
    fn detail_takes_precedence_over_the_capped_output() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0),
            tool_finished_full(
                2,
                thread,
                turn,
                100,
                "c1",
                "bash",
                false,
                50,
                "raw output body",
                Some("exit status 1"),
            ),
            completed(3, thread, turn, 200),
        ];
        let response = aggregate_trace_stats(thread, &events);
        assert_eq!(response.errors[0].detail.as_deref(), Some("exit status 1"));
    }

    #[test]
    fn slow_call_list_is_sorted_descending_and_capped() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let mut events = vec![started(1, thread, turn, 0)];
        // 12 calls with durations 1..=12 ms: only the ten slowest survive.
        for index in 1..=12u64 {
            let call = format!("c{index}");
            events.push(tool_started(
                index * 2,
                thread,
                turn,
                index as i64,
                &call,
                "bash",
                serde_json::json!({}),
            ));
            events.push(tool_finished(
                index * 2 + 1,
                thread,
                turn,
                index as i64 + 1,
                &call,
                "bash",
                true,
                index,
                "",
            ));
        }
        events.push(completed(30, thread, turn, 100));
        let response = aggregate_trace_stats(thread, &events);
        assert_eq!(response.tool_calls, 12);
        assert_eq!(response.slowest_calls.len(), TRACE_SLOW_CALLS_LIMIT);
        let durations: Vec<u64> = response
            .slowest_calls
            .iter()
            .map(|call| call.duration_ms)
            .collect();
        let mut sorted = durations.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(durations, sorted);
        assert_eq!(durations[0], 12);
        assert_eq!(durations[TRACE_SLOW_CALLS_LIMIT - 1], 3);
    }

    #[test]
    fn error_events_attach_to_the_failed_call_window() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0),
            tool_started(2, thread, turn, 100, "c1", "bash", serde_json::json!({})),
            tool_finished(3, thread, turn, 200, "c1", "bash", false, 100, ""),
            // Inside the failed call's window (before the next finish).
            error_at(4, thread, 250, "first"),
            tool_started(5, thread, turn, 300, "c2", "bash", serde_json::json!({})),
            tool_finished(6, thread, turn, 400, "c2", "bash", false, 100, ""),
            // After the last failed call: flushed onto the newest row.
            error_at(7, thread, 450, "second"),
            completed(8, thread, turn, 500),
        ];
        let response = aggregate_trace_stats(thread, &events);
        assert_eq!(response.errors.len(), 2);
        let first = &response.errors[0];
        assert_eq!(first.call_id, "c1");
        assert_eq!(first.errors.len(), 1);
        assert_eq!(first.errors[0].message, "first");
        let second = &response.errors[1];
        assert_eq!(second.call_id, "c2");
        assert_eq!(second.errors.len(), 1);
        assert_eq!(second.errors[0].message, "second");
    }

    #[test]
    fn successful_finish_closes_the_previous_error_window() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0),
            tool_finished(2, thread, turn, 100, "c1", "bash", false, 50, "boom"),
            tool_finished(3, thread, turn, 200, "c2", "bash", true, 50, "ok"),
            error_at(4, thread, 250, "must not attach backwards"),
            completed(5, thread, turn, 300),
        ];
        let response = aggregate_trace_stats(thread, &events);
        assert_eq!(response.errors.len(), 1);
        assert_eq!(response.errors[0].call_id, "c1");
        assert!(response.errors[0].errors.is_empty());
    }

    #[test]
    fn errors_without_a_failed_call_are_dropped() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0),
            tool_finished(2, thread, turn, 100, "c1", "bash", true, 50, ""),
            error_at(3, thread, 150, "orphan"),
            completed(4, thread, turn, 200),
        ];
        let response = aggregate_trace_stats(thread, &events);
        assert_eq!(response.failed_tool_calls, 0);
        assert!(response.errors.is_empty());
    }

    #[test]
    fn gate_wait_is_excluded_from_turn_active_ms() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0),
            tool_started(2, thread, turn, 100, "c1", "bash", serde_json::json!({})),
            RuntimeEvent::ApprovalRequested {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(200),
                approval_id: "a1".into(),
                tool: "bash".into(),
                summary: "x".into(),
            },
            RuntimeEvent::ApprovalResolved {
                seq: 4,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(62_000),
                approval_id: "a1".into(),
                approved: true,
            },
            tool_finished(5, thread, turn, 62_300, "c1", "bash", true, 100, ""),
            completed(6, thread, turn, 62_500),
        ];
        let response = aggregate_trace_stats(thread, &events);
        // The clock ticks on lifecycle and gate events only: 200 ms before
        // the gate plus 500 ms after the resolve; the parked hour stays out
        // and the tool events between add nothing.
        assert_eq!(response.turn_stats[0].active_ms, 700);
        assert_eq!(response.active_ms, Some(700));
    }

    #[test]
    fn tool_distribution_orders_by_call_count() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let mut events = vec![started(1, thread, turn, 0)];
        let mut seq = 2u64;
        for (tool, count) in [("read", 3), ("bash", 5), ("grep", 5)] {
            for index in 0..count {
                let call = format!("{tool}-{index}");
                events.push(tool_started(
                    seq,
                    thread,
                    turn,
                    0,
                    &call,
                    tool,
                    serde_json::json!({}),
                ));
                seq += 1;
                events.push(tool_finished(
                    seq, thread, turn, 1, &call, tool, true, 10, "",
                ));
                seq += 1;
            }
        }
        let response = aggregate_trace_stats(thread, &events);
        let names: Vec<&str> = response
            .tool_stats
            .iter()
            .map(|stat| stat.tool.as_str())
            .collect();
        // bash and grep tie at 5 calls and break on the tool name.
        assert_eq!(names, vec!["bash", "grep", "read"]);
        assert_eq!(response.tool_stats[0].calls, 5);
    }

    #[test]
    fn running_turn_stays_active_and_extends_to_the_newest_event() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0),
            usage_at(2, thread, turn, 500, 10),
        ];
        let response = aggregate_trace_stats(thread, &events);
        assert!(response.turn_stats[0].active);
        assert_eq!(response.turn_stats[0].ended_at, None);
        assert_eq!(response.turn_stats[0].active_ms, 500);
        assert_eq!(response.total_duration_ms, Some(500));
    }

    #[test]
    fn thread_span_uses_lifecycle_anchors_and_ignores_settled_trailing_events() {
        let thread = ThreadId::new();
        let first = TurnId::new();
        let second = TurnId::new();
        let events = vec![
            error_at(1, thread, -100, "before lifecycle"),
            started(2, thread, first, 100),
            usage_at(3, thread, first, 200, 10),
            completed(4, thread, first, 500),
            started(5, thread, second, 1_000),
            completed(6, thread, second, 1_500),
            error_at(7, thread, 9_000, "after terminal"),
        ];
        let response = aggregate_trace_stats(thread, &events);
        assert_eq!(response.total_duration_ms, Some(1_400));
    }

    #[test]
    fn thread_span_extends_only_a_final_running_turn_to_the_latest_event() {
        let thread = ThreadId::new();
        let first = TurnId::new();
        let second = TurnId::new();
        let events = vec![
            started(1, thread, first, 0),
            completed(2, thread, first, 500),
            started(3, thread, second, 1_000),
            usage_at(4, thread, second, 1_400, 10),
        ];
        assert_eq!(
            aggregate_trace_stats(thread, &events).total_duration_ms,
            Some(1_400)
        );
        assert_eq!(thread_span(&[started(1, thread, first, 250)]), Some(0));
    }
}
