//! Wire DTOs for the HTTP boundary (design appendix A / §7.9).
//!
//! These are transport shapes, not domain entities: they are what the renderer
//! and external clients see. Field names stay stable so a differential test can
//! compare the old runtime and `arkd` payload by payload.

use ark_common::{ThreadId, TurnId, Usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::item::ThreadItem;

/// `GET /health` — unauthenticated liveness probe.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    #[ts(as = "f64")]
    pub uptime_ms: u64,
}

/// `GET /v1/runtime/info`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct RuntimeInfo {
    pub runtime: String,
    pub version: String,
    pub data_dir: String,
    #[ts(as = "f64")]
    pub thread_count: u64,
    pub started_at: DateTime<Utc>,
}

/// One row of `GET /v1/threads`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ThreadSummary {
    pub thread_id: ThreadId,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub last_seq: Option<u64>,
    #[serde(default)]
    pub archived: bool,
}

/// `POST /v1/threads`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct CreateThreadRequest {
    #[serde(default)]
    pub title: Option<String>,
}

/// `PATCH /v1/threads/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct UpdateThreadRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub archived: Option<bool>,
}

/// `GET /v1/threads/{id}` — metadata plus the render-ready item stream.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ThreadDetail {
    pub summary: ThreadSummary,
    #[serde(default)]
    pub items: Vec<ThreadItem>,
    #[serde(default)]
    pub child_ids: Vec<ark_common::ChildId>,
}

/// `POST /v1/threads/{id}/turns`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TurnRequest {
    pub text: String,
}

/// `GET /v1/threads/{id}/turns/{turnId}`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TurnSummary {
    pub turn_id: TurnId,
    pub thread_id: ThreadId,
    pub state: String,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `GET /v1/usage`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct UsageResponse {
    #[ts(as = "f64")]
    pub turns: u64,
    #[serde(flatten)]
    pub usage: Usage,
}

// ---------------------------------------------------------------------------
// Child runs (design §7.8 / appendix A)
// ---------------------------------------------------------------------------
//
// Field names are camelCase because the renderer already parses these payloads
// (`ChildRunRecordJson` in `src/renderer/src/agent/types.ts`). Changing them here
// without changing the renderer silently blanks the sub-agent panel.

/// One row of `GET /v1/child-runs` and the body of `GET /v1/child-runs/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "camelCase")]
pub struct ChildRunSummary {
    pub id: ark_common::ChildId,
    pub parent_thread_id: ThreadId,
    pub parent_turn_id: TurnId,
    pub label: String,
    /// `queued` | `running` | `completed` | `failed` | `aborted`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Real counters the child reported; absent when it never reached a model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub tool_invocations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_count: Option<u32>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

/// Per-status counts plus the children's folded usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ChildRunAggregates {
    #[ts(as = "f64")]
    pub queued: u32,
    #[ts(as = "f64")]
    pub running: u32,
    #[ts(as = "f64")]
    pub completed: u32,
    #[ts(as = "f64")]
    pub failed: u32,
    #[ts(as = "f64")]
    pub aborted: u32,
    #[serde(default)]
    #[ts(as = "f64")]
    pub tool_invocations: u64,
    #[serde(default)]
    pub usage: Usage,
}

/// `GET /v1/child-runs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "camelCase")]
pub struct ChildRunListResponse {
    pub enabled: bool,
    /// Children holding a concurrency slot right now.
    #[ts(as = "f64")]
    pub active: u32,
    pub child_runs: Vec<ChildRunSummary>,
    pub aggregates: ChildRunAggregates,
}

/// `POST /v1/child-runs/{id}/steer`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SteerChildRequest {
    pub text: String,
}

/// `POST /v1/child-runs/{id}/resume`. `text` is the optional correction.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ResumeChildRequest {
    #[serde(default)]
    pub text: Option<String>,
}

/// Accepted-result body for abort/steer/resume/delete, so a client never has to
/// guess whether the call did anything.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "camelCase")]
pub struct ChildRunActionResult {
    pub id: ark_common::ChildId,
    /// Echo of the new state: `aborted`, `running`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_count: Option<u32>,
}

/// `GET /v1/child-runs/{id}/events` — incremental transcript polling.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "camelCase")]
pub struct ChildRunEventsResponse {
    pub events: Vec<crate::event::RuntimeEvent>,
    #[ts(as = "f64")]
    pub highest_seq: u64,
}
