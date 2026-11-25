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
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct TurnSpan {
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
struct ActiveClock {
    /// Spans per turn, keyed in first-seen order by the caller.
    turns: HashMap<TurnId, TurnSpan>,
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
    fn observe(&mut self, event: &RuntimeEvent) {
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
    fn finish(mut self) -> ActiveSpan {
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

/// Result of the [`ActiveClock`] fold: per-turn spans plus the thread total.
#[derive(Debug, Default)]
struct ActiveSpan {
    turns: HashMap<TurnId, TurnSpan>,
    total_ms: u64,
}

/// Timestamp of any event that can advance the clock's "now" horizon.
///
/// Turn lifecycle and gate events drive the spans themselves; `Usage` counts
/// too because a running turn's horizon must track the newest *anything* —
/// for a live fold that is what makes the clock advance between terminal
/// events. Pure rendering noise (deltas, items) stays out: the horizon only
/// needs to be fresh to within one UI-visible event.
fn event_timestamp(event: &RuntimeEvent) -> Option<DateTime<Utc>> {
    match event {
        RuntimeEvent::TurnStarted { timestamp, .. }
        | RuntimeEvent::TurnCompleted { timestamp, .. }
        | RuntimeEvent::TurnFailed { timestamp, .. }
        | RuntimeEvent::TurnAborted { timestamp, .. }
        | RuntimeEvent::ApprovalRequested { timestamp, .. }
        | RuntimeEvent::ApprovalResolved { timestamp, .. }
        | RuntimeEvent::UserInputRequested { timestamp, .. }
        | RuntimeEvent::UserInputResolved { timestamp, .. }
        | RuntimeEvent::Usage { timestamp, .. } => Some(*timestamp),
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
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use ark_common::{Result, Usage};
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig, StoreLimits};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::Utc;
    use tower::ServiceExt;

    use super::*;
    use crate::api::routes::router;
    use crate::state::{AppState, TurnEnd, TurnJob, TurnRunner};

    /// Backed by `tempfile`, whose random name keeps two parallel tests from
    /// picking the same path.
    struct TempDir(tempfile::TempDir);

    impl TempDir {
        fn new() -> Self {
            let dir = tempfile::Builder::new()
                .prefix("ark-diagnostics-test-")
                .tempdir()
                .expect("create test directory");
            Self(dir)
        }

        fn path(&self) -> &Path {
            self.0.path()
        }
    }

    struct EmptyTools;

    impl ToolRegistryPort for EmptyTools {
        fn names(&self) -> Vec<String> {
            Vec::new()
        }
        fn schemas(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }
        fn version(&self) -> u64 {
            0
        }
    }

    struct PendingRunner;

    #[async_trait::async_trait]
    impl TurnRunner for PendingRunner {
        async fn run_turn(&self, _job: TurnJob) -> Result<TurnEnd> {
            std::future::pending().await
        }
    }

    fn app(dir: &TempDir) -> (axum::Router, Arc<FileSessionStore>) {
        let store = Arc::new(
            FileSessionStore::open(
                StoreConfig::new(dir.path()).with_limits(StoreLimits::for_tests()),
            )
            .expect("open test store"),
        );
        let state = AppState::new(
            store.clone(),
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(PendingRunner),
            dir.path().to_path_buf(),
            "test",
        );
        (router(state), store)
    }

    fn at(offset_ms: i64) -> DateTime<Utc> {
        // Deterministic base so every call in a fixture shares one timeline.
        DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::milliseconds(offset_ms)
    }

    /// A `turn_started` event with all additive fields at their defaults.
    fn started(
        seq: u64,
        thread: ThreadId,
        turn: TurnId,
        at_ms: i64,
        recovery: u32,
    ) -> RuntimeEvent {
        RuntimeEvent::TurnStarted {
            auto_recovery: recovery,
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

    fn usage_at(seq: u64, thread: ThreadId, turn: TurnId, at_ms: i64) -> RuntimeEvent {
        RuntimeEvent::Usage {
            seq,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(at_ms),
            usage: Usage::new(10, 5),
        }
    }

    #[test]
    fn active_ms_equals_wall_clock_when_no_gate_opened() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0, 0),
            usage_at(2, thread, turn, 100),
            completed(3, thread, turn, 5_000),
        ];
        let response = aggregate_thread_usage(thread, &events);
        assert_eq!(response.turn_usage[0].active_ms, Some(5_000));
        assert_eq!(response.turn_usage[0].gate_suspensions, Some(0));
        assert_eq!(response.active_ms, Some(5_000));
    }

    #[test]
    fn active_ms_excludes_the_user_gate_window() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0, 0),
            usage_at(2, thread, turn, 1_000),
            // The user parks the approval for an hour of wall time.
            RuntimeEvent::ApprovalRequested {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(2_000),
                approval_id: "a1".into(),
                tool: "bash".into(),
                summary: "cargo test".into(),
            },
            RuntimeEvent::ApprovalResolved {
                seq: 4,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(2_000 + 3_600_000),
                approval_id: "a1".into(),
                approved: true,
            },
            usage_at(5, thread, turn, 2_000 + 3_600_000 + 500),
            completed(6, thread, turn, 2_000 + 3_600_000 + 1_500),
        ];
        let response = aggregate_thread_usage(thread, &events);
        // 0→2s active before the gate, then the parked hour excluded, then
        // 1.5s active after the resolve: 3.5s of the 3_603.5s wall clock.
        assert_eq!(response.turn_usage[0].active_ms, Some(3_500));
        assert_eq!(response.turn_usage[0].gate_suspensions, Some(1));
        assert_eq!(response.active_ms, Some(3_500));
    }

    #[test]
    fn user_input_gate_freezes_the_clock_too() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0, 0),
            RuntimeEvent::UserInputRequested {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(500),
                request_id: "u1".into(),
                prompt: "which db?".into(),
            },
            RuntimeEvent::UserInputResolved {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(62_500),
                request_id: "u1".into(),
                answer: "postgres".into(),
            },
            completed(4, thread, turn, 63_000),
        ];
        let response = aggregate_thread_usage(thread, &events);
        // 0.5s before the gate + 0.5s after it resolved.
        assert_eq!(response.turn_usage[0].active_ms, Some(1_000));
        assert_eq!(response.turn_usage[0].gate_suspensions, Some(1));
    }

    #[test]
    fn running_turn_extends_to_the_newest_event() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0, 0),
            usage_at(2, thread, turn, 2_500),
        ];
        let response = aggregate_thread_usage(thread, &events);
        // No terminal event yet: the span runs to the newest event seen,
        // which for a live fold is "now" to within one event.
        assert_eq!(response.turn_usage[0].active_ms, Some(2_500));
    }

    #[test]
    fn idle_gap_before_a_user_turn_is_not_counted() {
        let thread = ThreadId::new();
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let events = vec![
            started(1, thread, turn_a, 0, 0),
            completed(2, thread, turn_a, 1_000),
            // The user goes away for 30 minutes, then sends the next message.
            started(3, thread, turn_b, 1_000 + 1_800_000, 0),
            completed(4, thread, turn_b, 1_000 + 1_800_000 + 2_000),
        ];
        let response = aggregate_thread_usage(thread, &events);
        assert_eq!(response.turn_usage[0].active_ms, Some(1_000));
        assert_eq!(response.turn_usage[1].active_ms, Some(2_000));
        assert_eq!(response.active_ms, Some(3_000));
    }

    #[test]
    fn auto_recovery_backoff_rides_the_recovery_turn_span() {
        let thread = ThreadId::new();
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let events = vec![
            started(1, thread, turn_a, 0, 0),
            // The first turn dies on a model-request failure at t=1000.
            RuntimeEvent::TurnFailed {
                seq: 2,
                thread_id: thread,
                turn_id: turn_a,
                timestamp: at(1_000),
                message: "502".into(),
                model_request_failed: true,
            },
            // The harness retries 30s later as an auto_recovery turn.
            started(3, thread, turn_b, 31_000, 1),
            completed(4, thread, turn_b, 33_000),
        ];
        let response = aggregate_thread_usage(thread, &events);
        assert_eq!(response.turn_usage[0].active_ms, Some(1_000));
        // The failed attempt's span plus the backoff gap plus the retry's
        // span: the harness working, not the user thinking.
        assert_eq!(response.turn_usage[1].active_ms, Some(32_000));
        assert_eq!(response.active_ms, Some(33_000));
    }

    #[test]
    fn auto_continuation_gap_is_counted_like_recovery() {
        let thread = ThreadId::new();
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let events = vec![
            started(1, thread, turn_a, 0, 0),
            completed(2, thread, turn_a, 1_000),
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 3,
                thread_id: thread,
                turn_id: turn_b,
                timestamp: at(11_000),
                auto_continuation: true,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
            completed(4, thread, turn_b, 12_000),
        ];
        let response = aggregate_thread_usage(thread, &events);
        assert_eq!(response.turn_usage[1].active_ms, Some(11_000));
        assert_eq!(response.active_ms, Some(12_000));
    }

    #[test]
    fn a_turn_without_usage_still_reports_its_active_span() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        // A turn that failed before any model round produced no usage event;
        // its active time must still be visible on the clock.
        let events = vec![
            started(1, thread, turn, 0, 0),
            usage_at(2, thread, turn, 800),
            RuntimeEvent::TurnFailed {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(1_200),
                message: "boom".into(),
                model_request_failed: false,
            },
        ];
        let response = aggregate_thread_usage(thread, &events);
        assert_eq!(response.turn_usage.len(), 1);
        assert_eq!(response.turn_usage[0].turn_id, turn);
        assert_eq!(response.turn_usage[0].active_ms, Some(1_200));
        assert_eq!(response.samples, 1);
    }

    #[test]
    fn gate_open_at_terminal_counts_a_suspension_without_freezing() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 0, 0),
            RuntimeEvent::ApprovalRequested {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(500),
                approval_id: "a1".into(),
                tool: "bash".into(),
                summary: "x".into(),
            },
            RuntimeEvent::TurnAborted {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(600),
                reason: "user interrupt".into(),
            },
        ];
        let response = aggregate_thread_usage(thread, &events);
        // The gate froze the clock at 500ms, so the aborted turn's active
        // time is only the pre-gate window; the suspension counter records
        // that a gate was raised and never resolved.
        assert_eq!(response.turn_usage[0].active_ms, Some(500));
        assert_eq!(response.turn_usage[0].gate_suspensions, Some(1));
    }

    #[test]
    fn clock_skew_never_pushes_active_ms_negative() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            started(1, thread, turn, 1_000, 0),
            // A resolved timestamp earlier than it opened (skewed writer).
            RuntimeEvent::ApprovalRequested {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(2_000),
                approval_id: "a1".into(),
                tool: "bash".into(),
                summary: "x".into(),
            },
            RuntimeEvent::ApprovalResolved {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(1_500),
                approval_id: "a1".into(),
                approved: true,
            },
            completed(4, thread, turn, 2_500),
        ];
        let response = aggregate_thread_usage(thread, &events);
        // The skewed resolve (earlier than the gate opened) cannot rewind the
        // cursor into banked time: the clock restarts where it froze, so the
        // span is 1s before the gate + 0.5s after the terminal bank — never a
        // double count, never a negative.
        assert_eq!(response.turn_usage[0].active_ms, Some(1_500));
        assert_eq!(response.turn_usage[0].gate_suspensions, Some(1));
    }

    #[test]
    fn thread_usage_aggregates_cache_counters_across_turns() {
        let thread = ThreadId::new();
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let events = vec![
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 1,
                thread_id: thread,
                turn_id: turn_a,
                timestamp: at(0),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
            RuntimeEvent::Usage {
                seq: 2,
                thread_id: thread,
                turn_id: turn_a,
                timestamp: at(10),
                usage: Usage::new(100, 20).with_cache(Some(80), Some(20)),
            },
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 3,
                thread_id: thread,
                turn_id: turn_b,
                timestamp: at(20),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
            RuntimeEvent::Usage {
                seq: 4,
                thread_id: thread,
                turn_id: turn_b,
                timestamp: at(30),
                usage: Usage::new(50, 10),
            },
        ];

        let response = aggregate_thread_usage(thread, &events);
        assert_eq!(response.thread_id, thread);
        assert_eq!(response.turns, 2);
        assert_eq!(response.samples, 2);
        assert_eq!(response.usage.prompt_tokens, 150);
        assert_eq!(response.usage.completion_tokens, 30);
        // One sample without cache counters keeps the aggregated rate unknown
        // instead of inventing a definite hit rate from mixed evidence.
        assert_eq!(response.usage.cache_hit_tokens, Some(80));
        assert_eq!(response.usage.cache_hit_rate, None);
        // The per-turn breakdown is the same fold in miniature, so the turn
        // rows always add up to the thread totals.
        assert_eq!(response.turn_usage.len(), 2);
        assert_eq!(response.turn_usage[0].turn_id, turn_a);
        assert_eq!(response.turn_usage[0].samples, 1);
        assert_eq!(response.turn_usage[1].turn_id, turn_b);
        let folded_prompt: u64 = response
            .turn_usage
            .iter()
            .map(|turn| turn.usage.prompt_tokens)
            .sum();
        assert_eq!(folded_prompt, response.usage.prompt_tokens);
    }

    #[test]
    fn thread_usage_keeps_the_latest_valid_prompt_reading() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let usage_event = |seq, usage| RuntimeEvent::Usage {
            seq,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(seq as i64),
            usage,
        };
        let mut events = vec![
            started(1, thread, turn, 0, 0),
            usage_event(2, Usage::default()),
        ];
        assert_eq!(
            aggregate_thread_usage(thread, &events).last_valid_prompt_tokens,
            None
        );

        events.push(usage_event(3, Usage::new(120, 30)));
        assert_eq!(
            aggregate_thread_usage(thread, &events).last_valid_prompt_tokens,
            Some(120)
        );

        events.push(usage_event(4, Usage::default()));
        events.push(usage_event(5, Usage::new(150, 40)));
        events.push(usage_event(6, Usage::default()));
        let response = aggregate_thread_usage(thread, &events);
        assert_eq!(response.samples, 5);
        assert_eq!(response.usage.prompt_tokens, 270);
        assert_eq!(response.last_valid_prompt_tokens, Some(150));
    }

    #[test]
    fn per_turn_usage_keeps_every_turn_regardless_of_page_bounds() {
        // The regression behind the "122 steps in 11 seconds" report: the
        // per-turn clock used to be re-derived from the *paginated*
        // diagnostics route, so once a session exceeded one page the newest
        // turns fell off the page and their clocks collapsed. The usage
        // aggregate replays the whole log, so it must report every turn's
        // measured time no matter how many rounds came before it.
        let thread = ThreadId::new();
        let mut events = Vec::new();
        let mut turns = Vec::new();
        for index in 0..40u64 {
            let turn = TurnId::new();
            turns.push(turn);
            events.push(RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: index * 4,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(index as i64 * 100),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            });
            for round in 0..2u64 {
                let mut usage = Usage::new(10, 1);
                // Each round is measured; the newest turn's span must survive.
                usage.duration_ms = Some(1_500);
                events.push(RuntimeEvent::Usage {
                    seq: index * 4 + round + 1,
                    thread_id: thread,
                    turn_id: turn,
                    timestamp: at(index as i64 * 100 + round as i64 + 1),
                    usage,
                });
            }
        }

        let response = aggregate_thread_usage(thread, &events);
        assert_eq!(response.turns, 40);
        assert_eq!(response.turn_usage.len(), 40);
        let last = response.turn_usage.last().expect("newest turn");
        assert_eq!(last.turn_id, *turns.last().unwrap());
        assert_eq!(last.samples, 2);
        // Two measured rounds of 1.5s each: the whole span, not a remainder.
        assert_eq!(last.usage.duration_ms, Some(3_000));
        // And the thread total still equals the sum of its turns.
        let folded: u64 = response
            .turn_usage
            .iter()
            .map(|turn| turn.usage.duration_ms.unwrap_or(0))
            .sum();
        assert_eq!(Some(folded), response.usage.duration_ms);
    }

    #[test]
    fn rounds_cover_the_whole_call_window_and_attach_errors() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(0),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
            // An error while the first model call is still in flight.
            RuntimeEvent::Error {
                seq: 2,
                thread_id: thread,
                timestamp: at(5),
                code: ArkErrorCode::ModelRequestFailed,
                message: "provider hiccup".into(),
            },
            RuntimeEvent::Usage {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(10),
                usage: Usage::new(10, 5),
            },
            RuntimeEvent::Usage {
                seq: 4,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(40),
                usage: Usage::new(20, 10),
            },
            // After the final round: attached to the last round at turn end.
            RuntimeEvent::Error {
                seq: 5,
                thread_id: thread,
                timestamp: at(45),
                code: ArkErrorCode::ModelRequestFailed,
                message: "late failure".into(),
            },
            RuntimeEvent::TurnCompleted {
                seq: 6,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(50),
            },
        ];

        let rounds = derive_rounds(&events, None);
        assert_eq!(rounds.len(), 2);
        assert_eq!(rounds[0].round, 1);
        assert_eq!(rounds[0].started_at, at(0));
        assert_eq!(rounds[0].ended_at, at(10));
        assert_eq!(rounds[0].usage.duration_ms, Some(10));
        assert_eq!(rounds[0].errors.len(), 1);
        assert_eq!(rounds[0].errors[0].message, "provider hiccup");
        assert_eq!(rounds[1].round, 2);
        assert_eq!(rounds[1].started_at, at(10));
        assert_eq!(rounds[1].usage.duration_ms, Some(30));
        assert_eq!(rounds[1].errors.len(), 1);
        assert_eq!(rounds[1].errors[0].message, "late failure");
    }

    #[test]
    fn measured_round_duration_wins_over_the_boundary_window() {
        // The loop measures the model call itself. A long tool execution
        // between rounds must not inflate the reported model time, and a
        // pre-measurement log (no `usage.duration_ms`) falls back to the
        // window between round boundaries.
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let mut measured = Usage::new(10, 5);
        measured.duration_ms = Some(1_200);
        let events = vec![
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(0),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
            RuntimeEvent::Usage {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(10),
                usage: measured,
            },
            RuntimeEvent::Usage {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(40),
                usage: Usage::new(20, 10),
            },
        ];

        let rounds = derive_rounds(&events, None);
        assert_eq!(rounds.len(), 2);
        assert_eq!(
            rounds[0].usage.duration_ms,
            Some(1_200),
            "measured model time wins"
        );
        assert_eq!(
            rounds[1].usage.duration_ms,
            Some(30),
            "unmeasured samples fall back to the window"
        );
    }

    #[test]
    fn a_turn_without_usage_produces_no_round() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![RuntimeEvent::TurnStarted {
            auto_recovery: 0,
            seq: 1,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(0),
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        }];
        assert!(derive_rounds(&events, None).is_empty());
    }

    #[tokio::test]
    async fn usage_route_aggregates_from_the_event_log() {
        let dir = TempDir::new();
        let (app, store) = app(&dir);
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/threads")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap();
        let thread: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let thread_id: ThreadId = thread["thread_id"]
            .as_str()
            .expect("thread id")
            .parse()
            .expect("parse thread id");

        let turn = TurnId::new();
        store
            .append_event(
                thread_id,
                RuntimeEvent::TurnStarted {
                    auto_recovery: 0,
                    seq: 0,
                    thread_id,
                    turn_id: turn,
                    timestamp: Utc::now(),
                    auto_continuation: false,
                    attachment_ids: Vec::new(),
                    attachment_outcomes: Vec::new(),
                },
            )
            .await
            .unwrap();
        store
            .append_event(
                thread_id,
                RuntimeEvent::Usage {
                    seq: 0,
                    thread_id,
                    turn_id: turn,
                    timestamp: Utc::now(),
                    usage: Usage::new(120, 30).with_cache(Some(90), Some(30)),
                },
            )
            .await
            .unwrap();
        store
            .append_event(
                thread_id,
                RuntimeEvent::Usage {
                    seq: 0,
                    thread_id,
                    turn_id: turn,
                    timestamp: Utc::now(),
                    usage: Usage::default(),
                },
            )
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/threads/{thread_id}/usage"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["thread_id"], thread_id.to_string());
        assert_eq!(json["turns"], 1);
        assert_eq!(json["samples"], 2);
        assert_eq!(json["prompt_tokens"], 120);
        assert_eq!(json["cache_hit_tokens"], 90);
        assert_eq!(json["last_valid_prompt_tokens"], 120);
    }

    #[tokio::test]
    async fn llm_rounds_route_is_bounded_and_filterable() {
        let dir = TempDir::new();
        let (app, store) = app(&dir);
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/threads")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap();
        let thread: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let thread_id: ThreadId = thread["thread_id"]
            .as_str()
            .expect("thread id")
            .parse()
            .expect("parse thread id");

        for round_no in 0..3u64 {
            let turn = TurnId::new();
            store
                .append_event(
                    thread_id,
                    RuntimeEvent::TurnStarted {
                        auto_recovery: 0,
                        seq: 0,
                        thread_id,
                        turn_id: turn,
                        timestamp: at(round_no as i64 * 100),
                        auto_continuation: false,
                        attachment_ids: Vec::new(),
                        attachment_outcomes: Vec::new(),
                    },
                )
                .await
                .unwrap();
            store
                .append_event(
                    thread_id,
                    RuntimeEvent::Usage {
                        seq: 0,
                        thread_id,
                        turn_id: turn,
                        timestamp: at(round_no as i64 * 100 + 50),
                        usage: Usage::new(10, 1),
                    },
                )
                .await
                .unwrap();
        }

        // limit=2 keeps the route bounded.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/debug/llm-rounds?limit=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["rounds"].as_array().unwrap().len(), 2);
        assert_eq!(json["truncated"], true);

        // thread filter resolves the same rounds for the explicit thread.
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/v1/debug/llm-rounds?thread_id={thread_id}&limit=10"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["rounds"].as_array().unwrap().len(), 3);
        assert_eq!(json["truncated"], false);
    }

    #[tokio::test]
    async fn unknown_thread_answers_structured_not_found() {
        let dir = TempDir::new();
        let (app, _store) = app(&dir);
        let missing = ThreadId::new();
        for uri in [
            format!("/v1/threads/{missing}/usage"),
            format!("/v1/debug/llm-rounds?thread_id={missing}"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    // The unused `event` helper is kept out of the build; drop the stub that
    // referenced it instead of fighting the type inference of a generic fn path.
}
