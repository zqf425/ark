//! Wire types for `GET /v1/threads/{id}/trace-stats` (requirements/trace-analysis.md).
//!
//! Everything here is a pure projection of one thread's authoritative event
//! log — no LLM span events (they stay `PLANNED_KINDS`), no second copy of any
//! usage or tool state. Aggregation itself lives in `ark-server`
//! (`handlers/trace_stats.rs`); this crate only fixes the shapes.

use ark_common::{ArkErrorCode, ThreadId, TurnId, Usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum number of slow calls reported by one trace-stats response.
///
/// Exported so the settings chart caps at the same boundary the server
/// truncates, instead of hard-coding a second copy of the number.
pub const TRACE_SLOW_CALLS_LIMIT: usize = 10;

/// One failed tool call with the runtime errors observed in its window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TraceErrorEntry {
    /// Sequence number of the `tool_call_finished` row, so the web table can
    /// jump back into the thread timeline.
    #[ts(as = "f64")]
    pub seq: u64,
    pub turn_id: TurnId,
    pub call_id: String,
    pub tool: String,
    pub timestamp: DateTime<Utc>,
    #[ts(as = "f64")]
    pub duration_ms: u64,
    /// Short failure reason (`detail`, else the capped output) for the list row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `RuntimeEvent::Error` records observed in the same window (from this
    /// call's finish to the next tool finish or the turn's terminal event,
    /// whichever comes first); each carries its own timestamp.
    #[serde(default)]
    pub errors: Vec<TraceRuntimeError>,
}

/// One `RuntimeEvent::Error` projected into a trace-stats error list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TraceRuntimeError {
    #[ts(as = "f64")]
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub code: ArkErrorCode,
    pub message: String,
}

/// Slowest tool calls of the thread, `duration_ms` descending.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TraceSlowCall {
    #[ts(as = "f64")]
    pub seq: u64,
    pub turn_id: TurnId,
    pub call_id: String,
    pub tool: String,
    pub timestamp: DateTime<Utc>,
    #[ts(as = "f64")]
    pub duration_ms: u64,
    /// Arguments summary, truncated to a bounded prefix for the list row; the
    /// full arguments stay on the thread's event log.
    #[serde(default)]
    pub arguments_summary: String,
}

/// One turn's per-turn trace readout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TurnStat {
    pub turn_id: TurnId,
    pub started_at: DateTime<Utc>,
    /// Timestamp of the turn's terminal event (`turn_completed` /
    /// `turn_failed` / `turn_aborted`); a still-running turn leaves this
    /// unset and the response reports it as active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// Wall-clock span of the turn with user-gate windows excluded, folded by
    /// the same `ActiveClock` projection that backs `TurnUsage.active_ms`, so
    /// the two readouts can never disagree about one turn.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    #[ts(as = "f64")]
    pub active_ms: u64,
    /// `true` while the turn has no terminal event in the log.
    #[serde(default)]
    pub active: bool,
    /// Completed model rounds inside this turn (one `usage` event each).
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    #[ts(as = "f64")]
    pub model_rounds: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    #[ts(as = "f64")]
    pub tool_calls: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    #[ts(as = "f64")]
    pub failed_tool_calls: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// Per-tool aggregate across the whole thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ToolStat {
    pub tool: String,
    #[ts(as = "f64")]
    pub calls: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    #[ts(as = "f64")]
    pub failures: u64,
    /// Mean `duration_ms` over finished calls only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub avg_duration_ms: Option<u64>,
}

/// `GET /v1/threads/{id}/trace-stats`.
///
/// Session-level readout for the settings "trace analysis" section, projected
/// from the thread's event log (`replay_events`) on every request: nothing is
/// persisted, so a refresh after new turns simply reports new numbers.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TraceStatsResponse {
    pub thread_id: ThreadId,
    /// Turns observed in the log (`turn_started` occurrences, first-seen
    /// order), including ones that failed before their first model round.
    #[ts(as = "f64")]
    pub turns: u64,
    /// First `turn_started` to the last terminal event, milliseconds; a
    /// running thread extends to its newest event. The raw wall-clock span,
    /// user waits included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub total_duration_ms: Option<u64>,
    /// Sum of the per-turn gate-excluded spans (same fold as
    /// `ThreadUsageResponse.active_ms`), so this readout and `turn_stats`
    /// add up by construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub active_ms: Option<u64>,
    /// Model calls, counted as one `usage` event per completed round.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    #[ts(as = "f64")]
    pub model_calls: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    #[ts(as = "f64")]
    pub tool_calls: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    #[ts(as = "f64")]
    pub failed_tool_calls: u64,
    /// Days with at least one durable event, derived from event timestamps.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    #[ts(as = "f64")]
    pub active_days: u64,
    /// Newest durable event timestamp; drives the "updated" readout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub turn_stats: Vec<TurnStat>,
    /// Per-tool aggregates ordered by call count (descending, tool name as
    /// tiebreak) so the chart is stable across reloads.
    #[serde(default)]
    pub tool_stats: Vec<ToolStat>,
    /// Slowest finished calls, `duration_ms` descending, capped at
    /// `TRACE_SLOW_CALLS_LIMIT`.
    #[serde(default)]
    pub slowest_calls: Vec<TraceSlowCall>,
    /// Failed calls with their correlated `RuntimeEvent::Error` window, log
    /// order preserved.
    #[serde(default)]
    pub errors: Vec<TraceErrorEntry>,
    /// Log watermark covered by this projection (`None` for an empty log).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub last_seq: Option<u64>,
}
