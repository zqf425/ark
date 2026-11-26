//! Per-thread usage aggregation and LLM round diagnostics.
//!
//! Both projections derive from the authoritative event log: the agent loop
//! appends exactly one `RuntimeEvent::Usage` per completed model round, so a
//! round boundary, the per-thread totals, and the cache counters all come from
//! the same source the renderer replays. Nothing here keeps a second copy of
//! usage state, which is what keeps `GET /v1/usage`, the per-thread route, and
//! `GET /v1/debug/llm-rounds` consistent across restarts.

use std::collections::HashMap;

use ark_common::{ArkErrorCode, ThreadId, TurnId, Usage};
use ark_contract::{
    LlmRound, LlmRoundError, LlmRoundsResponse, RuntimeEvent, ThreadUsageResponse, TurnUsage,
    LLM_ROUNDS_DEFAULT_LIMIT, LLM_ROUNDS_MAX_LIMIT, LLM_ROUNDS_MAX_THREADS,
};
use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::state::AppState;

/// `GET /v1/threads/{id}/usage`.
///
/// Aggregated from the thread's persisted `usage` events, so it survives
/// restarts and reports exactly what the provider sent, including native cache
/// counters when present.
pub async fn thread_usage(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadUsageResponse>, ApiError> {
    let thread = parse_thread(&id)?;
    state.store.get_thread(thread).await?;
    let events = state.store.replay_events(thread, 0).await?;
    Ok(Json(aggregate_thread_usage(thread, &events)))
}

/// `GET /v1/debug/llm-rounds`.
///
/// Bounded on purpose: `limit` clamps to `[0, LLM_ROUNDS_MAX_LIMIT]`, and
/// without a `thread_id` the scan covers at most `LLM_ROUNDS_MAX_THREADS`
/// most recently updated threads, so the route can never walk the whole store.
pub async fn llm_rounds(
    State(state): State<AppState>,
    Query(query): Query<LlmRoundsQuery>,
) -> Result<Json<LlmRoundsResponse>, ApiError> {
    let limit = query
        .limit
        .unwrap_or(LLM_ROUNDS_DEFAULT_LIMIT)
        .min(LLM_ROUNDS_MAX_LIMIT) as usize;
    let offset = query.offset.unwrap_or(0) as usize;
    let turn_filter = match query.turn_id.as_deref() {
        Some(raw) => Some(parse_turn(raw)?),
        None => None,
    };

    let threads: Vec<ThreadId> = match query.thread_id.as_deref() {
        Some(raw) => {
            let thread = parse_thread(raw)?;
            // Fail fast on an unknown thread instead of answering empty rounds.
            state.store.get_thread(thread).await?;
            vec![thread]
        }
        None => state
            .store
            .list_threads()
            .await?
            .into_iter()
            .take(LLM_ROUNDS_MAX_THREADS)
            .map(|meta| meta.thread_id)
            .collect(),
    };

    let mut rounds = Vec::new();
    for thread in threads {
        let events = state.store.replay_events(thread, 0).await?;
        rounds.extend(derive_rounds(&events, turn_filter));
    }
    rounds.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.thread_id.to_string().cmp(&b.thread_id.to_string()))
            .then_with(|| a.turn_id.to_string().cmp(&b.turn_id.to_string()))
            .then_with(|| a.round.cmp(&b.round))
    });
    let truncated = rounds.len() > offset.saturating_add(limit);
    let rounds = rounds.into_iter().skip(offset).take(limit).collect();
    Ok(Json(LlmRoundsResponse { rounds, truncated }))
}

#[derive(Debug, Deserialize)]
pub struct LlmRoundsQuery {
    /// Restrict the diagnostics to one thread.
    #[serde(default)]
    pub thread_id: Option<String>,
    /// Further restrict to one turn inside `thread_id`.
    #[serde(default)]
    pub turn_id: Option<String>,
    /// Page size, clamped to `[0, LLM_ROUNDS_MAX_LIMIT]` when present.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Zero-based offset into the deterministic round ordering.
    #[serde(default)]
    pub offset: Option<u32>,
}

fn parse_turn(raw: &str) -> Result<TurnId, ApiError> {
    raw.parse::<TurnId>().map_err(|err| {
        ApiError::new(
            ArkErrorCode::ValidationError,
            format!("`{raw}` is not a valid turn id: {err}"),
        )
    })
}

/// Fold every persisted `usage` event of one thread into the wire totals.
///
/// The per-turn breakdown is built in the same single pass, so the thread
/// totals and the turn rows can never disagree: a turn's row is exactly the
/// fold of the samples counted into the thread total. Turns keep first-seen
/// order (the log is append-only, so that is also chronological).
///
/// The same pass derives the wall-clock `active_ms` readout (user waits
/// excluded) from the turn-lifecycle and gate events, so a renderer never
/// re-times anything from its own clock. See [`ActiveClock`].
fn aggregate_thread_usage(thread: ThreadId, events: &[RuntimeEvent]) -> ThreadUsageResponse {
    let mut samples = 0u64;
    let mut usage = ark_common::Usage::default();
    let mut last_valid_prompt_tokens = None;
    // First-seen order over all turns, including those that failed before their
    // first model round but still have active time for the clock to cover.
    let mut order: Vec<TurnId> = Vec::new();
    let mut seen: std::collections::HashSet<TurnId> = std::collections::HashSet::new();
    let mut per_turn: HashMap<TurnId, TurnUsage> = HashMap::new();
    let mut clock = ActiveClock::default();
    for event in events {
        if let RuntimeEvent::TurnStarted { turn_id, .. } = event {
            if seen.insert(*turn_id) {
                order.push(*turn_id);
            }
        }
        clock.observe(event);
        if let RuntimeEvent::Usage {
            turn_id,
            usage: sample,
            ..
        } = event
        {
            samples += 1;
            if sample.prompt_tokens > 0 {
                last_valid_prompt_tokens = Some(sample.prompt_tokens);
            }
            usage.merge(sample);
            match per_turn.get_mut(turn_id) {
                Some(row) => {
                    row.samples += 1;
                    row.usage.merge(sample);
                }
                None => {
                    per_turn.insert(
                        *turn_id,
                        TurnUsage {
                            turn_id: *turn_id,
                            samples: 1,
                            usage: sample.clone(),
                            active_ms: None,
                            gate_suspensions: None,
                        },
                    );
                }
            }
        }
    }
    let span = clock.finish();
    let turn_usage: Vec<TurnUsage> = order
        .into_iter()
        .map(|turn_id| {
            // A usage-less turn folds to the default (zero tokens, no measured
            // durations): its row exists for the active clock, and the token
            // totals still add up because it contributes nothing.
            let mut row = per_turn.remove(&turn_id).unwrap_or(TurnUsage {
                turn_id,
                samples: 0,
                usage: ark_common::Usage::default(),
                active_ms: None,
                gate_suspensions: None,
            });
            if let Some(per_turn_span) = span.turns.get(&turn_id) {
                row.active_ms = Some(per_turn_span.active_ms);
                row.gate_suspensions = Some(per_turn_span.gate_suspensions);
            }
            row
        })
        .collect();
    ThreadUsageResponse {
        thread_id: thread,
        turns: span.turns.len() as u64,
        samples,
        last_seq: events.last().map(RuntimeEvent::seq),
        usage,
        last_valid_prompt_tokens,
        active_ms: Some(span.total_ms),
        gate_suspensions: Some(total_gate_suspensions(&turn_usage)),
        turn_usage,
    }
}

fn total_gate_suspensions(turns: &[TurnUsage]) -> u64 {
    turns.iter().filter_map(|turn| turn.gate_suspensions).sum()
}

/// Per-turn active-time span derived from the lifecycle and gate events.
///
/// Fields stay private; readers go through the accessor methods, which keeps
/// the cross-handler reuse surface (`trace_stats`) down to the readouts
/// themselves.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TurnSpan {
    active_ms: u64,
    gate_suspensions: u64,
}

/// Wall-clock accounting that treats user-caused waits as excluded time.
///
/// The clock runs as an open interval: while a turn is active (and no gate is
/// open) it accumulates `[cursor, event)` into that turn's span on every
/// lifecycle event and moves the cursor forward. An approval / user-input gate
/// **stops** the accumulation at its open event and **restarts** it at the
/// resolve event, so the user's thinking time never enters any span. The gap
/// between two turns is accumulated **only** when the harness itself inserted
/// it — an `auto_recovery` or `auto_continuation` `turn_started` re-opens the
/// clock from the previous turn's end (the backoff between retries is the
/// harness working, not the user thinking). A plain user / scheduled / IM turn
/// starts a fresh span: the idle time before it is the user composing the
/// message and must not appear on any clock.
///
/// A turn that never reached a terminal event (a crash, or one still running)
/// measures up to the last event timestamp seen: for a live thread that is
/// "now" to within one event, and for a dead log it is the honest last word.
#[derive(Debug, Default)]
pub(crate) struct ActiveClock {
    /// Spans per turn, keyed in first-seen order by the caller.
    pub(crate) turns: HashMap<TurnId, TurnSpan>,
    /// The turn currently on the clock, if any.
    open_turn_id: Option<TurnId>,
    /// When the currently open turn is a harness retry/continuation, the end
    /// of the previous turn: the inter-turn gap rides this turn's span.
    carried_from: Option<DateTime<Utc>>,
    /// Where the running clock last ticked. `None` while a gate holds the
    /// clock frozen or no turn is open.
    cursor: Option<DateTime<Utc>>,
    /// Whether a user gate currently holds the clock frozen.
    open_gate: bool,
    /// Newest event timestamp observed; the live-turn "now" for `finish`.
    last_event_at: Option<DateTime<Utc>>,
    /// End timestamp of the most recent settled turn; the anchor a harness
    /// retry carries its inter-turn gap from.
    last_turn_end: Option<DateTime<Utc>>,
}

impl ActiveClock {
    pub(crate) fn observe(&mut self, event: &RuntimeEvent) {
        if let Some(timestamp) = event_timestamp(event) {
            self.last_event_at = Some(timestamp);
        }
        match event {
            RuntimeEvent::TurnStarted {
                turn_id,
                timestamp,
                auto_continuation,
                auto_recovery,
                ..
            } => {
                self.open_turn_id = Some(*turn_id);
                self.turns.entry(*turn_id).or_default();
                self.open_gate = false;
                // Harness-initiated turns continue the working span from the
                // previous turn's end: the backoff gap rides *this* turn's
                // span, so bank it before the cursor moves to the turn start.
                // Every other source starts fresh: the idle time before it is
                // the user composing the message.
                let carried = (*auto_continuation || *auto_recovery > 0)
                    .then_some(self.last_turn_end)
                    .flatten();
                if let Some(from) = carried {
                    self.cursor = Some(from);
                    self.bank(*timestamp);
                }
                self.carried_from = carried;
                self.cursor = Some(*timestamp);
            }
            RuntimeEvent::ApprovalRequested { timestamp, .. }
            | RuntimeEvent::UserInputRequested { timestamp, .. } => {
                // Freeze the clock at the gate: the elapsed window since the
                // cursor banks into the turn, then nothing accumulates until
                // the user answers. Counting at open (not resolve) keeps the
                // counter honest even when the turn dies with the gate raised.
                self.bank(*timestamp);
                self.open_gate = true;
                if let Some(turn_id) = self.open_turn_id {
                    if let Some(span) = self.turns.get_mut(&turn_id) {
                        span.gate_suspensions += 1;
                    }
                }
            }
            RuntimeEvent::ApprovalResolved { timestamp, .. }
            | RuntimeEvent::UserInputResolved { timestamp, .. } => {
                // The frozen window itself stays excluded; restart the clock.
                // A skewed resolve earlier than the open would rewind the
                // cursor into already-banked time and double count, so the
                // cursor only ever moves forward.
                self.cursor = Some(self.cursor.map_or(*timestamp, |held| held.max(*timestamp)));
                self.open_gate = false;
            }
            RuntimeEvent::TurnCompleted { timestamp, .. }
            | RuntimeEvent::TurnFailed { timestamp, .. }
            | RuntimeEvent::TurnAborted { timestamp, .. } => {
                if !self.open_gate {
                    self.bank(*timestamp);
                }
                self.open_gate = false;
                self.open_turn_id = None;
                self.cursor = None;
                self.carried_from = None;
                self.last_turn_end = Some(*timestamp);
            }
            _ => {}
        }
    }

    /// Accumulate `[cursor, at)` into the open turn and move the cursor.
    ///
    /// No-op when the clock is not running (no open turn, or a gate holds it
    /// frozen). The elapsed window is clamped at zero, so a skewed timestamp
    /// can only stall the clock, never rewind it.
    fn bank(&mut self, at: DateTime<Utc>) {
        let (Some(turn_id), Some(from)) = (self.open_turn_id, self.cursor) else {
            return;
        };
        let Some(span) = self.turns.get_mut(&turn_id) else {
            return;
        };
        span.active_ms += Self::window_ms(from, at);
        // The cursor also only moves forward: a banked timestamp earlier than
        // the cursor (skewed writer) must not rewind it into banked time.
        self.cursor = Some(self.cursor.map_or(at, |held| held.max(at)));
    }

    /// Milliseconds in `[from, to)`, clamped at zero against clock skews.
    fn window_ms(from: DateTime<Utc>, to: DateTime<Utc>) -> u64 {
        (to - from).num_milliseconds().max(0) as u64
    }

    /// Finish the fold: extend the still-running turn to the last event seen.
    ///
    /// A gate held open at this point keeps the clock frozen: the window from
    /// the gate's open event to "now" is exactly the user wait the readout
    /// exists to exclude.
    pub(crate) fn finish(mut self) -> ActiveSpan {
        if !self.open_gate {
            if let Some(at) = self.last_event_at {
                self.bank(at);
            }
        }
        let total_ms = self.turns.values().map(|span| span.active_ms).sum();
        ActiveSpan {
            turns: self.turns,
            total_ms,
        }
    }
}

impl TurnSpan {
    /// Folded wall-clock time of this turn, in milliseconds.
    pub(crate) fn active_ms(&self) -> u64 {
        self.active_ms
    }
}

/// Result of the [`ActiveClock`] fold: per-turn spans plus the thread total.
#[derive(Debug, Default)]
pub(crate) struct ActiveSpan {
    pub(crate) turns: HashMap<TurnId, TurnSpan>,
    pub(crate) total_ms: u64,
}

/// Turn lifecycle, gate, usage and tool events advance the running horizon.
///
/// Only lifecycle and gate events drive the spans themselves; usage and tool
/// timestamps keep a running turn's final `finish` horizon fresh. Pure
/// rendering noise (deltas, items) stays out because it need not extend active
/// execution time.
pub(crate) fn event_timestamp(event: &RuntimeEvent) -> Option<DateTime<Utc>> {
    match event {
        RuntimeEvent::TurnStarted { timestamp, .. }
        | RuntimeEvent::TurnCompleted { timestamp, .. }
        | RuntimeEvent::TurnFailed { timestamp, .. }
        | RuntimeEvent::TurnAborted { timestamp, .. }
        | RuntimeEvent::ApprovalRequested { timestamp, .. }
        | RuntimeEvent::ApprovalResolved { timestamp, .. }
        | RuntimeEvent::UserInputRequested { timestamp, .. }
        | RuntimeEvent::UserInputResolved { timestamp, .. }
        | RuntimeEvent::Usage { timestamp, .. }
        | RuntimeEvent::ToolCallStarted { timestamp, .. }
        | RuntimeEvent::ToolCallFinished { timestamp, .. } => Some(*timestamp),
        _ => None,
    }
}

/// Per-turn accumulator for [`derive_rounds`].
struct TurnAccum {
    started_at: Option<DateTime<Utc>>,
    last_end: Option<DateTime<Utc>>,
    round: u64,
    pending_errors: Vec<(DateTime<Utc>, LlmRoundError)>,
    rounds: Vec<LlmRound>,
}

/// Close one model round: drain the errors recorded inside its window, defer
/// anything later, and push the finished round onto the accumulator.
fn close_round(
    acc: &mut TurnAccum,
    thread_id: ThreadId,
    turn_id: TurnId,
    ended_at: DateTime<Utc>,
    usage: &Usage,
) {
    let started_at = acc.last_end.or(acc.started_at).unwrap_or(ended_at);
    acc.round += 1;
    let round = acc.round;
    let mut errors = Vec::new();
    let mut deferred = Vec::new();
    for (at, error) in acc.pending_errors.drain(..) {
        if at <= ended_at {
            errors.push(error);
        } else {
            deferred.push((at, error));
        }
    }
    acc.pending_errors = deferred;
    // The loop measures the model call itself (usage.duration_ms); the window
    // between round boundaries is the fallback for logs written before the
    // field existed — it includes tool time, so it must not override a real
    // measurement. The value rides the flattened usage, which is the round's
    // single duration field on the wire.
    let mut round_usage = usage.clone();
    if round_usage.duration_ms.is_none() {
        round_usage.duration_ms = Some((ended_at - started_at).num_milliseconds().max(0) as u64);
    }
    acc.rounds.push(LlmRound {
        thread_id,
        turn_id,
        round,
        started_at,
        ended_at,
        errors,
        usage: round_usage,
    });
    acc.last_end = Some(ended_at);
}

/// Flush the errors a turn still holds when it ends.
///
/// Errors recorded after the final round still belong to that turn's last
/// round, because they happened while the runtime was working on it. A turn
/// without rounds simply drops its pending errors.
fn flush_turn_errors(acc: &mut TurnAccum) {
    if let Some(last) = acc.rounds.last_mut() {
        for (_, error) in acc.pending_errors.drain(..) {
            last.errors.push(error);
        }
    } else {
        acc.pending_errors.clear();
    }
}

/// Derive completed model rounds from the authoritative event log.
///
/// A round closes on the `Usage` event the loop appends right after a model
/// call settles; its window starts at the previous boundary inside the same
/// turn (the previous round's end, or `turn_started` for the first round).
/// Errors recorded inside a round's window attach to that round; errors after
/// the final round of a turn attach to that last round. A model call that dies
/// before settling produces no `Usage` event and therefore no round - the turn
/// failure stays visible through `TurnFailed`/`Error` events on the thread.
fn derive_rounds(events: &[RuntimeEvent], turn_filter: Option<TurnId>) -> Vec<LlmRound> {
    let mut turns: HashMap<TurnId, TurnAccum> = HashMap::new();
    let mut current: Option<TurnId> = None;

    for event in events {
        match event {
            RuntimeEvent::TurnStarted {
                turn_id, timestamp, ..
            } => {
                current = Some(*turn_id);
                turns.insert(
                    *turn_id,
                    TurnAccum {
                        started_at: Some(*timestamp),
                        last_end: None,
                        round: 0,
                        pending_errors: Vec::new(),
                        rounds: Vec::new(),
                    },
                );
            }
            RuntimeEvent::Usage {
                thread_id,
                turn_id,
                timestamp,
                usage,
                ..
            } => {
                if let Some(acc) = turns.get_mut(turn_id) {
                    close_round(acc, *thread_id, *turn_id, *timestamp, usage);
                }
            }
            RuntimeEvent::Error {
                timestamp,
                code,
                message,
                ..
            } => {
                if let Some(turn_id) = current {
                    if let Some(acc) = turns.get_mut(&turn_id) {
                        acc.pending_errors.push((
                            *timestamp,
                            LlmRoundError {
                                code: *code,
                                message: message.clone(),
                            },
                        ));
                    }
                }
            }
            RuntimeEvent::TurnCompleted { turn_id, .. }
            | RuntimeEvent::TurnFailed { turn_id, .. }
            | RuntimeEvent::TurnAborted { turn_id, .. } => {
                if let Some(acc) = turns.get_mut(turn_id) {
                    flush_turn_errors(acc);
                }
                if current == Some(*turn_id) {
                    current = None;
                }
            }
            _ => {}
        }
    }

    let mut all: Vec<LlmRound> = Vec::new();
    for acc in turns.into_values() {
        all.extend(acc.rounds);
    }
    if let Some(filter) = turn_filter {
        all.retain(|round| round.turn_id == filter);
    }
    all
}

#[cfg(test)]
mod tests;
