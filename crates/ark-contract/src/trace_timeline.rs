//! Wire types for `GET /v1/threads/{id}/trace-timeline`
//! (requirements/trace-analysis.md §v2).
//!
//! Like `trace_stats`, everything here is a pure projection of one thread's
//! authoritative event log — no second copy of any tool or usage state. The
//! fold itself lives in `ark-server` (`handlers/trace_timeline.rs`); this
//! crate only fixes the shapes.
//!
//! Timing honesty: exact LLM spans are still gated on `LlmSpanStart`/
//! `LlmSpanEnd` (planned, no producer), so a `model_call` span approximates
//! the round with the wall clock the agent loop recorded on its `usage`
//! event. Text items carry only a creation timestamp, so their
//! `duration_ms` stays unset instead of being inferred from the next row.

use ark_common::{ItemId, ThreadId, TurnId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum character count of a `text_snippet`.
///
/// Exported so the timeline panel caps at the same boundary the server
/// truncates, instead of hard-coding a second copy of the number.
pub const TRACE_TIMELINE_SNIPPET_MAX_CHARS: usize = 200;

/// What one timeline row represents.
///
/// The item-backed kinds mirror `ItemKind` (`assistant_text` instead of
/// `assistant_message` because the row describes model output, and
/// `tool_result` has no producer). `model_call` comes from `usage` events;
/// `compaction` covers both the checkpoint item and the compaction event
/// pair, which describe the same boundary from different angles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum TraceSpanKind {
    UserMessage,
    AssistantText,
    Reasoning,
    ToolCall,
    Approval,
    UserInput,
    Compaction,
    ModelCall,
    Error,
    Notice,
}

/// One span of a turn's timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TraceSpan {
    pub kind: TraceSpanKind,
    /// When the span began: the item's creation timestamp, except that tool
    /// calls use `ToolCallStarted` time (falling back to item creation when
    /// the start event is missing), compactions use `CompactionStarted` and
    /// model calls use the `usage` event timestamp.
    pub started_at: DateTime<Utc>,
    /// Real elapsed time, only where the log measures it: `tool_call` carries
    /// `ToolPayload.duration_ms`, `model_call` the agent-loop wall clock on
    /// the `usage` event, `compaction` the `CompactionStarted` →
    /// `CompactionCompleted` window. Text items have no end timestamp in the
    /// log, so their `duration_ms` is omitted rather than guessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub duration_ms: Option<u64>,
    /// Tool name for `tool_call`; for `model_call` the `provider:model` label
    /// the turn stamped on its opening user item (absent on older logs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Bounded text summary of at most `TRACE_TIMELINE_SNIPPET_MAX_CHARS`
    /// characters, cut on character boundaries so multi-byte text stays
    /// intact. Event bodies never ride the timeline — the full text stays on
    /// thread detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_snippet: Option<String>,
    /// `true` for failed tool calls and error rows; omitted (default false)
    /// so the happy path serializes nothing extra.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>")]
    pub is_error: bool,
    /// Item to jump back to in the session canvas. Rows derived straight from
    /// events (model calls, raw `error` events) have none; tool-call rows keep
    /// the id the events already carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<ItemId>,
}

/// `GET /v1/threads/{id}/trace-timeline?turn_id=…`.
///
/// Per-turn drill-down for the settings "trace analysis" section, projected
/// from the thread's event log on every request: nothing is persisted and
/// `spans` is the only readout. The server answers a structured `NotFound`
/// for a `turn_id` the log never started; a turn that exists but produced no
/// rows yields an empty `spans` list.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TraceTimeline {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    /// Timeline rows ordered by `started_at` ascending (stable sort, so the
    /// same log always projects the same order).
    #[serde(default)]
    pub spans: Vec<TraceSpan>,
    /// Log watermark covered by this projection (`None` for an empty log),
    /// same semantics as `TraceStatsResponse.last_seq`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub last_seq: Option<u64>,
}
