//! Wire types for `GET /v1/threads/{id}/trace-timeline`
//! (requirements/trace-analysis.md §v2).
//!
//! Like `trace_stats`, everything here is a pure projection of one thread's
//! authoritative event log -- no second copy of any tool or usage state. The
//! fold itself lives in `ark-server` (`handlers/trace_timeline.rs`); this
//! crate only fixes the shapes.
//!
//! Timing honesty: exact LLM spans are still gated on `LlmSpanStart`/
//! `LlmSpanEnd` (planned, no producer), so a `model_call` span approximates
//! the round with the wall clock the agent loop recorded on its `usage`
//! event. Text items carry only a creation timestamp: reasoning and assistant
//! messages are streamed *during* a model call, so they are not independent
//! timeline rows. They ride on the `model_call` span as `TraceSpanDetail`
//! entries instead, keeping the Gantt axis free of zero-duration text rows.
//!
//! Attachment rule: a text detail belongs to the model-call window
//! `[usage.timestamp - duration_ms, usage.timestamp]` that contains its
//! `created_at`, choosing the narrowest (shortest-duration) window when
//! windows overlap. Text outside every window is dropped -- notably on older
//! logs where `usage.duration_ms` is absent and no window can be reconstructed.
//! Dropped text stays available in the authoritative thread detail; only this
//! read-side projection loses it.

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

/// Text produced while a model call was streaming.
///
/// A `reasoning` or `assistant_message` item is a child event of the model
/// round, not an activity that occupies the timeline on its own: real logs
/// stamp it inside the round's `[end - duration, end]` window. Such items are
/// therefore never emitted as independent `TraceSpan` rows; they are attached
/// to the containing `model_call` row in ascending `started_at` order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum TraceSpanDetailKind {
    Reasoning,
    AssistantText,
}

/// Snapshot of the single `usage` event that produced a `model_call` row.
///
/// This mirrors `ark_common::Usage`'s wire fields only: no `merge` here, so a
/// row reports exactly what one provider round reported. Cache counters and
/// rate stay `None` when the provider omitted them -- never reconstructed
/// from prompt tokens in this read-side projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TraceSpanUsage {
    #[ts(as = "f64")]
    pub prompt_tokens: u64,
    #[ts(as = "f64")]
    pub completion_tokens: u64,
    #[ts(as = "f64")]
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub cache_hit_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub cache_miss_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Wall clock of this one call, copied from the event. `None` when the
    /// log was written before the runtime measured it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub duration_ms: Option<u64>,
    /// Copied from the event: `true` means the rate was derived from an
    /// inferred miss, not measured natively.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>")]
    pub cache_hit_rate_approximate: bool,
}

/// One streamed text fragment nested in its owning `model_call` span.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TraceSpanDetail {
    /// Which stream produced the text; `assistant_text` is the wire name of
    /// the runtime's `assistant_message` item.
    pub kind: TraceSpanDetailKind,
    /// Bounded summary of the item text, using the same character-boundary
    /// cap as timeline rows. The full body stays on thread detail.
    pub text_snippet: String,
    /// Item creation timestamp, which real logs place inside the owning model
    /// call's measured window.
    pub started_at: DateTime<Utc>,
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
    /// intact. Event bodies never ride the timeline -- the full text stays on
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
    /// Streamed reasoning / assistant text nested under this span, present on
    /// `model_call` rows only. Each fragment falls in this row's measured
    /// window and is ordered by `started_at` ascending; `None` means no
    /// text could be attributed (or this is not a model call). Text outside
    /// every model window -- including logs without `duration_ms` -- is dropped
    /// from this projection, not shown as its own row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<TraceSpanDetail>>,
    /// Token/cache snapshot from the `usage` event, `model_call` rows only.
    /// Cache fields are provider-optional: absent means unreported, never a
    /// value inferred by this projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TraceSpanUsage>,
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
