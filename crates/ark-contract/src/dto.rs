//! Wire DTOs for the HTTP boundary (design appendix A / §7.9).
//!
//! These are transport shapes, not domain entities: they are what the renderer
//! and external clients see. Field names stay stable so a differential test can
//! compare the old runtime and `arkd` payload by payload.

use ark_common::{ThreadId, TurnId, Usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{item::ThreadItem, ChildCompletionNotice};

/// Maximum UTF-8 byte length accepted for one turn request.
pub const MAX_TURN_TEXT_BYTES: usize = 1024 * 1024;

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

/// Default page size of `GET /v1/threads`.
pub const THREAD_PAGE_DEFAULT_LIMIT: u32 = 50;
/// Maximum page size of `GET /v1/threads`; larger values are clamped.
pub const THREAD_PAGE_MAX_LIMIT: u32 = 200;

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

/// `GET /v1/threads` query parameters.
///
/// Ordering is the store's deterministic order (active first, then
/// `updated_at` descending, thread id as final tiebreak); `offset` counts rows
/// in that order so a page never repeats or skips rows between calls.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ThreadListQuery {
    /// Page size, clamped to `[1, THREAD_PAGE_MAX_LIMIT]` when present.
    #[serde(default)]
    #[ts(as = "Option<f64>")]
    pub limit: Option<u32>,
    /// Zero-based row offset into the deterministic ordering.
    #[serde(default)]
    #[ts(as = "Option<f64>")]
    pub offset: Option<u32>,
    /// When true, return only archived threads; when false, only active ones;
    /// when absent, return all. Composes with `limit`/`offset` and `q`.
    #[serde(default)]
    pub archived: Option<bool>,
    /// Case-insensitive substring filter on the thread title; composes with
    /// `limit`/`offset` and `archived`.
    #[serde(default)]
    pub q: Option<String>,
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
    #[serde(default)]
    pub runtime: Option<ThreadRuntimeSettings>,
}

/// Per-thread overrides applied when the next turn is created.
///
/// Empty fields fall back to the runtime defaults. The values are metadata, not
/// secrets: model credentials remain exclusively in the provider table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ThreadRuntimeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
}

/// `GET /v1/threads/{id}` — metadata, items and durable child terminal summaries.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ThreadDetail {
    pub summary: ThreadSummary,
    #[serde(default)]
    pub items: Vec<ThreadItem>,
    #[serde(default)]
    pub child_ids: Vec<ark_common::ChildId>,
    #[serde(default)]
    pub child_completions: Vec<ChildCompletionNotice>,
    #[serde(default)]
    pub runtime: ThreadRuntimeSettings,
    /// Process-local state that is not persisted with the session settings.
    #[serde(default)]
    pub runtime_snapshot: ThreadRuntimeSnapshot,
}

/// Controls that exist only while the current arkd process owns the turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ThreadRuntimeSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<TurnSummary>,
    #[serde(default)]
    pub pending_approvals: Vec<PendingApprovalView>,
    #[serde(default)]
    pub pending_user_inputs: Vec<PendingUserInputView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct PendingApprovalView {
    pub approval_id: String,
    pub turn_id: TurnId,
    pub tool: String,
    pub summary: String,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct PendingUserInputView {
    pub request_id: String,
    pub turn_id: TurnId,
    pub prompt: String,
    pub requested_at: DateTime<Utc>,
}

/// `POST /v1/threads/{id}/turns`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TurnRequest {
    pub text: String,
}

/// `POST /v1/threads/{id}/rewind`.
///
/// Exactly one target must be provided. A turn target means the rewind starts
/// at that turn's `turn_started` event; a sequence target starts at that event.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct RewindRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub seq: Option<u64>,
}

/// Result of physically truncating a thread's authoritative event history.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct RewindResponse {
    #[ts(as = "f64")]
    pub last_seq: u64,
    #[ts(as = "f64")]
    pub rewind_from_seq: u64,
    #[serde(default)]
    pub already_rewound: bool,
}

/// `POST /v1/approvals/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ResolveApprovalRequest {
    pub approved: bool,
}

/// `POST /v1/user-inputs/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ResolveUserInputRequest {
    pub answer: String,
}

/// One task in the durable thread plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// User-maintained task shown in the Code plan panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TodoEntry {
    pub id: String,
    pub title: String,
    pub status: TodoStatus,
}

/// The current durable goal plus todo list of one thread.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct PlanView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default)]
    pub todos: Vec<TodoEntry>,
}

/// `POST /v1/threads/{id}/goal`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct GoalRequest {
    pub goal: String,
}

/// `POST /v1/threads/{id}/todos` replaces the complete ordered list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TodosRequest {
    #[serde(default)]
    pub todos: Vec<TodoEntry>,
}

/// One file mutation projected from a durable tool call.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct WorkspaceChange {
    pub path: String,
    pub operation: String,
    pub turn_id: TurnId,
    #[ts(as = "f64")]
    pub seq: u64,
}

/// `GET /v1/threads/{id}/changes`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct WorkspaceChangesResponse {
    #[serde(default)]
    pub changes: Vec<WorkspaceChange>,
}

/// A workspace known to the runtime. `path` is always the canonical root chosen
/// by the host, never an untrusted request string.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct WorkspaceDescriptor {
    pub path: String,
    #[ts(as = "f64")]
    pub generation: u64,
}

/// `GET /v1/workspace` — active workspace plus the bounded recent history.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct WorkspaceView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<WorkspaceDescriptor>,
    #[serde(default)]
    pub recent: Vec<WorkspaceDescriptor>,
    #[ts(as = "f64")]
    pub generation: u64,
}

/// `POST /v1/workspace/switch`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct SwitchWorkspaceRequest {
    pub path: String,
}

/// One Git status entry. `patch` is optional because binary and capped output
/// are represented explicitly rather than silently reading arbitrary files.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct WorkspaceDiffFile {
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    #[serde(default)]
    pub binary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

/// Bounded Git view for the active workspace. Staged, unstaged and untracked
/// are deliberately separate: session tool events are not a Git diff.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct WorkspaceDiff {
    #[ts(as = "f64")]
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub staged: Vec<WorkspaceDiffFile>,
    #[serde(default)]
    pub unstaged: Vec<WorkspaceDiffFile>,
    #[serde(default)]
    pub untracked: Vec<WorkspaceDiffFile>,
    #[ts(as = "f64")]
    pub file_count: u64,
    #[ts(as = "f64")]
    pub patch_bytes: u64,
    #[serde(default)]
    pub truncated: bool,
}

/// `GET /v1/threads/{id}/files?path=...`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct FilePreview {
    pub path: String,
    pub content: String,
    #[ts(as = "f64")]
    pub source_seq: u64,
}

/// One completed shell command projected from a durable tool call.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TerminalEntry {
    pub call_id: String,
    pub turn_id: TurnId,
    pub command: String,
    pub output: String,
    pub ok: bool,
    #[ts(as = "f64")]
    pub duration_ms: u64,
    #[ts(as = "f64")]
    pub seq: u64,
}

/// `GET /v1/threads/{id}/terminal`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TerminalEntriesResponse {
    #[serde(default)]
    pub entries: Vec<TerminalEntry>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
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
#[serde(rename_all = "camelCase")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_run_aggregates_use_camel_case_on_the_wire() {
        let payload = serde_json::to_value(ChildRunAggregates {
            tool_invocations: 3,
            ..ChildRunAggregates::default()
        })
        .unwrap();

        assert_eq!(payload.get("toolInvocations"), Some(&serde_json::json!(3)));
        assert!(payload.get("tool_invocations").is_none());
    }

    #[test]
    fn thread_detail_runtime_defaults_for_older_payloads() {
        let detail: ThreadDetail = serde_json::from_value(serde_json::json!({
            "summary": {"thread_id": ThreadId::new(), "title": "t", "created_at": Utc::now(), "updated_at": Utc::now(), "archived": false},
            "items": [], "child_ids": [], "child_completions": []
        }))
        .unwrap();
        assert!(detail.runtime_snapshot.active_turn.is_none());
        assert!(detail.runtime_snapshot.pending_approvals.is_empty());
    }
}
