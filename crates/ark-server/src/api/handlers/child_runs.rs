//! `child-runs` routes (design §7.8 / appendix A).
//!
//! Every handler here is a translation layer: parse the path and query, ask a
//! port, map the result to a DTO. The rules that matter — the concurrency
//! budget, what "resumable" means, which child belongs to which parent — live in
//! `ark-delegation`, so this file cannot drift from the foreground
//! `delegate_task` path.
//!
//! Two shape decisions are worth stating:
//!
//! * `parentThreadId` is **required** on every single-child route. A child id is
//!   a UUID and a leaked one must not be enough to read another thread's
//!   transcript, so ownership is always re-checked against the parent's run log.
//! * a record is folded to its **latest** entry. `children/runs.jsonl` is
//!   append-only, so `queued`/`running`/`completed` rows all exist for the same
//!   child; returning the raw log would make the panel show a stale status.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use ark_common::{ArkErrorCode, ChildId, ThreadId};
use ark_contract::{
    ChildRunActionResult, ChildRunAggregates, ChildRunEventsResponse, ChildRunListResponse,
    ChildRunSummary, ResumeChildRequest, SteerChildRequest, ThreadDetail,
};
use ark_ports::{child_status, ChildRunRecord, SubagentRegistry, SubagentRequest, SubagentRunner};

use crate::api::error::ApiError;
use crate::api::handlers::threads::{parse_thread, summary_of};
use crate::state::AppState;

/// Query shared by the list route.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildRunsQuery {
    /// Restrict the listing to one parent. Absent lists every known child.
    #[serde(default)]
    pub thread_id: Option<String>,
    /// Required on the single-child routes; ownership is checked against it.
    #[serde(default)]
    pub parent_thread_id: Option<String>,
}

/// Query of the incremental transcript route.
///
/// `since_seq` keeps its **snake_case** wire name: the renderer and the old
/// runtime both send `?since_seq=N`, and reading it as `sinceSeq` silently
/// turned every incremental poll into a full replay from seq 0.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildEventsQuery {
    #[serde(default, rename = "since_seq", alias = "sinceSeq")]
    pub since_seq: Option<u64>,
    #[serde(default)]
    pub parent_thread_id: Option<String>,
}

fn unavailable() -> ApiError {
    ApiError::new(
        ArkErrorCode::RuntimeUnavailable,
        "sub-agents are not available in this build",
    )
}

/// Registry, or a structured `unavailable` when delegation was not wired.
fn registry(state: &AppState) -> Result<&std::sync::Arc<dyn SubagentRegistry>, ApiError> {
    state.subagents.registry.as_ref().ok_or_else(unavailable)
}

fn runner(state: &AppState) -> Result<&std::sync::Arc<dyn SubagentRunner>, ApiError> {
    state.subagents.runner.as_ref().ok_or_else(unavailable)
}

/// Parse a child id, reporting a validation error instead of a 500.
fn parse_child(raw: &str) -> Result<ChildId, ApiError> {
    raw.trim().parse::<ChildId>().map_err(|_| {
        ApiError::new(
            ArkErrorCode::ValidationError,
            format!("`{raw}` is not a valid child id"),
        )
    })
}

/// The parent scope of a single-child route.
///
/// Required on purpose: a child id alone must never be enough to read a
/// transcript, because ids travel through logs and bug reports.
fn require_parent(query: &ChildRunsQuery) -> Result<ThreadId, ApiError> {
    let Some(raw) = query.parent_thread_id.as_deref() else {
        return Err(ApiError::new(
            ArkErrorCode::ValidationError,
            "parentThreadId is required for child-run access",
        ));
    };
    parse_thread(raw)
}

/// Fold append-only records to the latest row per child, newest first.
fn fold_latest(records: Vec<ChildRunRecord>) -> Vec<ChildRunRecord> {
    let mut latest: std::collections::HashMap<ChildId, ChildRunRecord> =
        std::collections::HashMap::new();
    for record in records {
        latest.insert(record.child_id, record);
    }
    let mut folded: Vec<ChildRunRecord> = latest.into_values().collect();
    // Newest first, with the id as a tie-break so the order is stable instead of
    // depending on hash iteration.
    folded.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.child_id.to_string().cmp(&b.child_id.to_string()))
    });
    folded
}

fn summary_of_record(record: &ChildRunRecord) -> ChildRunSummary {
    ChildRunSummary {
        id: record.child_id,
        parent_thread_id: record.parent_thread_id,
        parent_turn_id: record.parent_turn_id,
        label: record.label.clone(),
        status: record.status.clone(),
        summary: record.summary.clone(),
        error: record.error.clone(),
        model: record.model.clone(),
        provider_id: record.provider_id.clone(),
        usage: record.usage.clone(),
        tool_invocations: record.tool_invocations,
        resume_count: record.resume_count,
        created_at: record.created_at,
        ended_at: record.ended_at,
    }
}

/// Fold per-status counts. Only real counters are summed; a missing usage sample
/// stays missing rather than contributing a zero that looks measured.
fn aggregates_of(records: &[ChildRunRecord]) -> ChildRunAggregates {
    let mut aggregates = ChildRunAggregates::default();
    for record in records {
        match record.status.as_str() {
            child_status::QUEUED => aggregates.queued += 1,
            child_status::RUNNING => aggregates.running += 1,
            child_status::COMPLETED => aggregates.completed += 1,
            child_status::FAILED => aggregates.failed += 1,
            child_status::ABORTED => aggregates.aborted += 1,
            _ => {}
        }
        aggregates.tool_invocations += record.tool_invocations.unwrap_or(0);
        if let Some(usage) = &record.usage {
            aggregates.usage.merge(usage);
        }
    }
    aggregates
}

/// `GET /v1/child-runs`.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ChildRunsQuery>,
) -> Result<Json<ChildRunListResponse>, ApiError> {
    let registry = registry(&state)?;
    let parent = match query.thread_id.as_deref() {
        Some(raw) => Some(parse_thread(raw)?),
        None => None,
    };
    let records = fold_latest(registry.list(parent));
    let aggregates = aggregates_of(&records);
    Ok(Json(ChildRunListResponse {
        enabled: registry.is_enabled(),
        active: registry.active_count(),
        child_runs: records.iter().map(summary_of_record).collect(),
        aggregates,
    }))
}

/// `GET /v1/child-runs/{child_id}`.
pub async fn get(
    State(state): State<AppState>,
    Path(child_id): Path<String>,
    Query(query): Query<ChildRunsQuery>,
) -> Result<Json<ChildRunSummary>, ApiError> {
    let registry = registry(&state)?;
    let parent = require_parent(&query)?;
    let child = parse_child(&child_id)?;
    let record = registry
        .get(child)
        .filter(|record| record.parent_thread_id == parent)
        .ok_or_else(|| not_found(child))?;
    Ok(Json(summary_of_record(&record)))
}

/// `POST /v1/child-runs/{child_id}/abort`.
pub async fn abort(
    State(state): State<AppState>,
    Path(child_id): Path<String>,
    Query(query): Query<ChildRunsQuery>,
) -> Result<Json<ChildRunActionResult>, ApiError> {
    let registry = registry(&state)?;
    let parent = require_parent(&query)?;
    let child = parse_child(&child_id)?;
    ensure_owned(registry, child, parent)?;
    if !registry.abort(child) {
        // Not running: the run either already finished or never started. Saying
        // "aborted" here would be a lie the UI then renders as a terminal state.
        return Err(ApiError::new(
            ArkErrorCode::NotFound,
            format!("child run {child} is not running"),
        ));
    }
    Ok(Json(ChildRunActionResult {
        id: child,
        status: Some(child_status::ABORTED.to_string()),
        ok: None,
        deleted: None,
        resume_count: None,
    }))
}

/// `POST /v1/child-runs/{child_id}/steer`.
pub async fn steer(
    State(state): State<AppState>,
    Path(child_id): Path<String>,
    Query(query): Query<ChildRunsQuery>,
    Json(body): Json<SteerChildRequest>,
) -> Result<Json<ChildRunActionResult>, ApiError> {
    let registry = registry(&state)?;
    let parent = require_parent(&query)?;
    let child = parse_child(&child_id)?;
    let text = body.text.trim();
    if text.is_empty() {
        return Err(ApiError::new(
            ArkErrorCode::ValidationError,
            "steer text must not be empty",
        ));
    }
    ensure_owned(registry, child, parent)?;
    // `false` means "not running, or its queue is full". Reporting a silent
    // success would drop the user's correction.
    if !registry.steer(child, text) {
        return Err(ApiError::new(
            ArkErrorCode::Conflict,
            format!("child run {child} cannot accept steering right now"),
        ));
    }
    Ok(Json(ChildRunActionResult {
        id: child,
        status: None,
        ok: Some(true),
        deleted: None,
        resume_count: None,
    }))
}

/// `POST /v1/child-runs/{child_id}/resume`.
///
/// Answers immediately with the queued record: a resumed run may take minutes,
/// so holding the connection would look like a hang. Everything decidable now
/// (unknown child, already completed, still running, disabled) is still an
/// error here, so the client never gets an accepted-but-doomed resume.
pub async fn resume(
    State(state): State<AppState>,
    Path(child_id): Path<String>,
    Query(query): Query<ChildRunsQuery>,
    body: Option<Json<ResumeChildRequest>>,
) -> Result<Json<ChildRunActionResult>, ApiError> {
    let runner = std::sync::Arc::clone(runner(&state)?);
    let registry = registry(&state)?;
    let parent = require_parent(&query)?;
    let child = parse_child(&child_id)?;
    ensure_owned(registry, child, parent)?;

    let correction = body
        .and_then(|Json(request)| request.text)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let prompt = correction.clone().unwrap_or_else(|| {
        // No correction supplied: replay the original task. The record still
        // carries it, which is why `prompt` is persisted.
        registry
            .get(child)
            .and_then(|record| record.prompt)
            .unwrap_or_default()
    });
    if prompt.trim().is_empty() {
        return Err(ApiError::new(
            ArkErrorCode::ValidationError,
            format!("child run {child} has no recorded prompt to resume; supply `text`"),
        ));
    }

    let request = SubagentRequest {
        parent_thread_id: parent,
        parent_turn_id: registry
            .get(child)
            .map(|record| record.parent_turn_id)
            .unwrap_or_default(),
        label: registry
            .get(child)
            .map(|record| record.label)
            .unwrap_or_default(),
        prompt,
        workspace: None,
        model: None,
        provider_id: None,
        resume_child_id: Some(child),
    };

    // The runtime installs the abort handle and owns the supervisor before this
    // returns. The HTTP handler therefore cannot strand a queued resume by
    // dropping an unobserved JoinHandle.
    let resumed = runner.delegate_background(request).await?;
    let resume_count = registry.get(resumed).and_then(|record| record.resume_count);

    Ok(Json(ChildRunActionResult {
        id: resumed,
        status: Some(child_status::QUEUED.to_string()),
        ok: None,
        deleted: None,
        resume_count,
    }))
}

/// `DELETE /v1/child-runs/{child_id}`.
///
/// Destroys the run record and its session directory. Aborting is the way to
/// stop a run while keeping its transcript; this is the only destructive path.
pub async fn remove(
    State(state): State<AppState>,
    Path(child_id): Path<String>,
    Query(query): Query<ChildRunsQuery>,
) -> Result<Json<ChildRunActionResult>, ApiError> {
    let registry = registry(&state)?;
    let parent = require_parent(&query)?;
    let child = parse_child(&child_id)?;
    ensure_owned(registry, child, parent)?;
    // Destruction is a runtime lifecycle operation, not a raw store mutation:
    // the runtime refuses active runs and prevents a late finish from
    // resurrecting a record that was just deleted.
    if !registry.destroy_child(parent, child).await? {
        return Err(not_found(child));
    }
    Ok(Json(ChildRunActionResult {
        id: child,
        status: None,
        ok: None,
        deleted: Some(true),
        resume_count: None,
    }))
}

/// `GET /v1/child-runs/{child_id}/thread` — the child's transcript.
///
/// The child session lives at `threads/<parent>/children/<child>/`, which the
/// store resolves from the run log, so the same item projection as a top-level
/// thread is reused. `latestSeq` is what the detail panel polls from.
pub async fn thread(
    State(state): State<AppState>,
    Path(child_id): Path<String>,
    Query(query): Query<ChildRunsQuery>,
) -> Result<Json<ThreadDetail>, ApiError> {
    let registry = registry(&state)?;
    let parent = require_parent(&query)?;
    let child = parse_child(&child_id)?;
    ensure_owned(registry, child, parent)?;
    let thread_id = ThreadId::from_uuid(child.as_uuid());
    let meta = state.store.get_thread(thread_id).await?;
    let items = state.store.read_items(thread_id).await?;
    let mut summary = summary_of(&meta);
    summary.last_seq = state.store.highest_seq(thread_id).await.ok();
    Ok(Json(ThreadDetail {
        summary,
        items,
        child_ids: Vec::new(),
        child_completions: Vec::new(),
        runtime: meta.runtime,
        runtime_snapshot: ark_contract::ThreadRuntimeSnapshot::default(),
    }))
}

/// `GET /v1/child-runs/{child_id}/events` — incremental transcript polling.
pub async fn events(
    State(state): State<AppState>,
    Path(child_id): Path<String>,
    Query(query): Query<ChildEventsQuery>,
) -> Result<Json<ChildRunEventsResponse>, ApiError> {
    let parent = match query.parent_thread_id.as_deref() {
        Some(raw) => parse_thread(raw)?,
        None => {
            return Err(ApiError::new(
                ArkErrorCode::ValidationError,
                "parentThreadId is required for child-run access",
            ))
        }
    };
    let registry = registry(&state)?;
    let child = parse_child(&child_id)?;
    ensure_owned(registry, child, parent)?;
    let thread_id = ThreadId::from_uuid(child.as_uuid());
    let since = query.since_seq.unwrap_or(0);
    let events = state.store.replay_events(thread_id, since).await?;
    let highest = state.store.highest_seq(thread_id).await?;
    Ok(Json(ChildRunEventsResponse {
        events,
        highest_seq: highest,
    }))
}

fn not_found(child: ChildId) -> ApiError {
    ApiError::new(
        ArkErrorCode::NotFound,
        format!("child run not found: {child}"),
    )
}

/// Ownership check shared by every single-child route.
///
/// Reads the authoritative run log through the registry, so a hand-edited
/// `index.json` cannot grant access to a child that the log does not claim.
fn ensure_owned(
    registry: &std::sync::Arc<dyn SubagentRegistry>,
    child: ChildId,
    parent: ThreadId,
) -> Result<(), ApiError> {
    match registry.get(child) {
        Some(record) if record.parent_thread_id == parent => Ok(()),
        // A child under another parent is reported as missing rather than
        // forbidden: answering 403 would confirm that the id exists.
        _ => Err(not_found(child)),
    }
}

/// Unused import guard: `StatusCode` documents that these routes never hand-roll
/// a status code; the error code decides it. Kept referenced so a future edit
/// cannot silently start writing statuses by hand.
const _: Option<StatusCode> = None;

#[cfg(test)]
mod tests;
