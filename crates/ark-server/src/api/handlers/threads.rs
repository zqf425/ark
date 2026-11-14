//! Thread routes (design §7.9): list, create, get, update, delete.
//!
//! The store is the system of record (design §7.4), so these handlers only
//! translate DTOs and publish the matching lifecycle event. No thread state is
//! cached here.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;

use ark_common::ThreadId;
use ark_contract::{
    BulkDeleteThreadsResponse, ChildCompletionNotice, CreateThreadRequest, PendingApprovalView,
    PendingUserInputView, RuntimeEvent, ThreadDetail, ThreadListQuery, ThreadRuntimeSnapshot,
    ThreadSummary, UpdateThreadRequest, THREAD_PAGE_MAX_LIMIT,
};
use std::collections::HashSet;

use crate::api::error::ApiError;
use crate::service::summary_of;
use crate::state::AppState;

/// Placeholder title when the client sends none.
pub const DEFAULT_TITLE: &str = "New thread";
const MAX_CHILD_COMPLETION_NOTICES: usize = 50;

/// `DELETE /v1/threads` query parameters (bulk per-project delete).
#[derive(Debug, Default, serde::Deserialize)]
pub struct ThreadWorkspaceQuery {
    /// Project workspace path; required, matched against each thread's
    /// effective workspace override.
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Resolve each listed thread's durable "turn in flight" verdict.
///
/// Delegates to the shared fold in the service layer; the handler only maps
/// the result onto list rows.
async fn durable_running_ids(
    state: &AppState,
    registry_active: &HashSet<ThreadId>,
    candidates: impl IntoIterator<Item = ThreadId>,
) -> HashSet<ThreadId> {
    crate::service::threads::durable_running_ids(state, registry_active, candidates).await
}

/// `GET /v1/threads`.
///
/// Returns a bounded page of the store's deterministic ordering (active first,
/// then most recently updated, thread id as tiebreak). Absent query parameters
/// keep the legacy behaviour of listing every thread, so the response stays a
/// bare array; clients paginate with `limit` + `offset`, and an empty page
/// marks the end.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ThreadListQuery>,
) -> Result<Json<Vec<ThreadSummary>>, ApiError> {
    // Absent `limit` is the legacy "list everything" mode; clamping it to the
    // page default would truncate existing clients. Only an explicit `limit`
    // enables paging, and only it is clamped.
    let limit = query
        .limit
        .map(|limit| limit.clamp(1, THREAD_PAGE_MAX_LIMIT));
    let offset = query.offset.unwrap_or(0);
    let needle = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());

    let metas = state.store.list_threads().await?;
    // Deterministic filter-then-page: sorting here (instead of trusting index
    // order alone) keeps pagination stable across calls, because the tiebreak
    // is the immutable id. The activity key mirrors the index's own `sorted`:
    // last durable event first, metadata update time as the fallback, so a
    // rename or settings edit never outranks a thread that just streamed.
    let mut rows: Vec<&ark_ports::ThreadMeta> = metas
        .iter()
        .filter(|meta| query.archived.is_none_or(|want| meta.archived == want))
        .filter(|meta| needle.is_none_or(|n| meta.title.to_lowercase().contains(&n.to_lowercase())))
        .collect();
    rows.sort_by(|a, b| {
        a.archived
            .cmp(&b.archived)
            .then_with(|| {
                b.last_event_at
                    .or(Some(b.updated_at))
                    .cmp(&a.last_event_at.or(Some(a.updated_at)))
            })
            .then_with(|| a.thread_id.cmp(&b.thread_id))
    });

    // One registry read per page, not per row: the lock is short but shared
    // with turn registration. Rows the registry does not know about fall back
    // to the durable log's newest turn-lifecycle event (restarts, turns
    // started by other processes), so the busy marker survives a restart.
    let registry_running = state.turns.active_thread_ids();
    let ids: Vec<ThreadId> = rows.iter().map(|meta| meta.thread_id).collect();
    let running = durable_running_ids(&state, &registry_running, ids).await;
    let summaries = rows
        .iter()
        .skip(offset as usize)
        .take(limit.map_or(usize::MAX, |limit| limit as usize))
        .map(|meta| {
            let mut summary = summary_of(meta);
            summary.running = running.contains(&meta.thread_id);
            summary
        })
        .collect();
    Ok(Json(summaries))
}

/// `POST /v1/threads`.
///
/// An explicit `workspace` is validated through `WorkspaceService::validate`
/// before any store write (the same rule as the composer's pick), so the rail's
/// per-project "new session" action can never seed a dead override; the
/// validated path is normalized by the workspace service itself.
pub async fn create(
    State(state): State<AppState>,
    body: Option<Json<CreateThreadRequest>>,
) -> Result<(StatusCode, Json<ThreadSummary>), ApiError> {
    let request = body.map(|Json(request)| request).unwrap_or_default();
    let title = request
        .title
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_TITLE.to_string());
    let workspace = match request.workspace.as_deref().map(str::trim) {
        Some(path) if !path.is_empty() => {
            let service = state.base.workspace.as_ref().ok_or_else(|| {
                ApiError::new(
                    ark_common::ArkErrorCode::RuntimeUnavailable,
                    "workspace service is not available in this build",
                )
            })?;
            Some(
                service
                    .validate(path)
                    .map_err(ApiError::from)?
                    .to_string_lossy()
                    .into_owned(),
            )
        }
        _ => None,
    };
    let summary =
        crate::service::threads::create_thread(&state, &title, workspace.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(summary)))
}

/// `GET /v1/threads/{id}`.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadDetail>, ApiError> {
    let thread_id = parse_thread(&id)?;
    Ok(Json(load_detail(&state, thread_id).await?))
}

/// Build the full wire detail for one thread.
///
/// Shared by `GET /v1/threads/{id}` and `POST /v1/sessions/{id}/resume-thread`
/// so a resumed session observes exactly what a thread read observes.
pub(crate) async fn load_detail(
    state: &AppState,
    thread_id: ThreadId,
) -> Result<ThreadDetail, ApiError> {
    let meta = state.store.get_thread(thread_id).await?;
    let items = state.store.read_items(thread_id).await?;
    let child_completions = recent_child_completions(
        state
            .store
            .replay_child_completion_events(thread_id)
            .await?,
    );
    // `children/runs.jsonl` is append-only, so one child owns a row per status
    // change (`queued`, `running`, the terminal row, plus one row per resume).
    // Mapping the raw log straight to ids listed the same child once per
    // transition. Fold first: this is the same latest-row-per-child rule the
    // derived `children/index.json` and `GET /v1/child-runs` already apply.
    let child_ids = state
        .store
        .list_child_runs(thread_id)
        .await
        .map(|records| {
            ark_ports::latest_child_runs(records)
                .into_iter()
                .map(|record| record.child_id)
                .collect()
        })
        .unwrap_or_default();
    let mut summary = summary_of(&meta);
    summary.last_seq = state.store.highest_seq(thread_id).await.ok();
    // The detail row answers the same "is this thread busy" question the list
    // row does; reading the registry once here keeps both views consistent.
    summary.running = state.turns.active_for(thread_id).is_some();
    Ok(ThreadDetail {
        summary,
        items,
        child_ids,
        child_completions,
        // Projected from this thread's own log, so a resume and a plain read
        // cannot disagree about which attachments the thread may reference.
        attachment_ids: crate::service::thread_attachment_ids(state, thread_id).await?,
        runtime: meta.runtime,
        runtime_snapshot: project_runtime_snapshot(
            state.turns.active_for(thread_id),
            state.store.replay_interaction_events(thread_id).await?,
            state.gates.pending_approval_ids(),
            state.gates.pending_user_input_ids(),
        ),
    })
}

/// `PATCH /v1/threads/{id}`.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateThreadRequest>,
) -> Result<Json<ThreadSummary>, ApiError> {
    let thread_id = parse_thread(&id)?;
    let mut meta = state.store.get_thread(thread_id).await?;
    if let Some(title) = body.title {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                "title must not be empty",
            ));
        }
        meta = state.store.rename_thread(thread_id, trimmed).await?;
        let event = RuntimeEvent::ThreadUpdated {
            seq: 0,
            thread_id,
            timestamp: Utc::now(),
            title: meta.title.clone(),
        };
        state.store.append_event(thread_id, event).await?;
    }
    if let Some(archived) = body.archived {
        let _workspace_guard = state.workspace_gate.lock().await;
        meta = state.store.set_archived(thread_id, archived).await?;
        if archived {
            state.continuations.suppress(thread_id);
        } else {
            state.continuations.allow(thread_id);
            crate::service::schedule_auto_continuation(state.clone(), thread_id).await;
        }
    }
    if let Some(runtime) = body.runtime {
        let merged = merge_runtime(meta.runtime, runtime, state.base.providers.as_deref())?;
        meta = state.store.set_thread_runtime(thread_id, merged).await?;
    }
    Ok(Json(summary_of(&meta)))
}

fn merge_runtime(
    current: ark_contract::ThreadRuntimeSettings,
    patch: ark_contract::ThreadRuntimeSettings,
    providers: Option<&dyn ark_ports::ProviderService>,
) -> Result<ark_contract::ThreadRuntimeSettings, ApiError> {
    let autonomy = patch.autonomy.or(current.autonomy);
    if let Some(value) = autonomy.as_deref() {
        // Same vocabulary as the settings layer; the terser message predates it
        // and is kept so existing clients' assertions do not shift.
        if ark_common::Autonomy::parse(value).is_err() {
            return Err(ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                "invalid autonomy",
            ));
        }
    }
    let reasoning_effort = patch.reasoning_effort.or(current.reasoning_effort);
    if let Some(value) = reasoning_effort.as_deref() {
        if !value.is_empty() && !ark_contract::REASONING_EFFORTS.contains(&value) {
            return Err(ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                "invalid reasoning effort; use off, low, medium or high",
            ));
        }
    }
    let provider_id = patch.provider_id.or(current.provider_id);
    let model = patch.model.or(current.model);
    if provider_id.is_some() || model.as_deref().is_some_and(|value| !value.is_empty()) {
        let providers = providers.ok_or_else(|| {
            ApiError::new(
                ark_common::ArkErrorCode::RuntimeUnavailable,
                "provider service is not available in this build",
            )
        })?;
        if providers.resolve_endpoint(provider_id.as_deref()).is_none() {
            let message = provider_id
                .as_deref()
                .map(|id| format!("thread provider is unavailable: {id}"))
                .unwrap_or_else(|| "thread model requires an available provider".to_string());
            return Err(ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                message,
            ));
        }
    }
    Ok(ark_contract::ThreadRuntimeSettings {
        workspace: patch.workspace.or(current.workspace),
        provider_id,
        model,
        autonomy,
        reasoning_effort,
    })
}

/// `DELETE /v1/threads/{id}`.
///
/// Refuses while a turn is still running. The store deletes the directory and
/// forgets the writer, but a live reactor keeps publishing: its next append
/// would recreate a directory with events but no `thread.json`, i.e. a thread
/// that cannot be listed, opened or cleaned up (design §7.4.1, P1-4).
///
/// Returns `409` so the caller can interrupt first and retry, matching the
/// existing "reject a second turn on a busy thread" policy.
pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let thread_id = parse_thread(&id)?;
    let _workspace_guard = state.workspace_gate.lock().await;
    // Validate ownership and active state before changing continuation state. A
    // failed delete must not suppress future work for a thread that still exists.
    state.store.get_thread(thread_id).await?;
    if let Some(active) = state.turns.active_for(thread_id) {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::Conflict,
            format!(
                "thread has a running turn ({}) and cannot be deleted; interrupt it first",
                active.turn_id
            ),
        ));
    }
    let was_suppressed = state.continuations.suppress(thread_id);
    if let Err(error) = state.store.delete_thread(thread_id).await {
        if !was_suppressed {
            state.continuations.allow(thread_id);
            crate::service::schedule_auto_continuation(state.clone(), thread_id).await;
        }
        return Err(error.into());
    }
    // Thread ids are never reused. Drop the process-local tombstone after the
    // durable deletion; late child wakeups fail the store existence check.
    // Also reclaim the turn-registry epoch marker: it is insert-only on
    // registration and would otherwise leak one entry per deleted thread.
    state.continuations.allow(thread_id);
    state.turns.forget_thread(thread_id);
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/threads/{id}/fork`.
///
/// A fork always uses the current durable watermark. Forking from a selected
/// historical sequence needs an explicit history UI; accepting an arbitrary
/// number here would expose a partial feature with no way for the renderer to
/// explain which prefix it selected.
pub async fn fork(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<ThreadSummary>), ApiError> {
    let thread_id = parse_thread(&id)?;
    state.store.get_thread(thread_id).await?;
    let watermark = state.store.highest_seq(thread_id).await?;
    let forked = state.store.fork_thread(thread_id, watermark).await?;
    let mut summary = summary_of(&forked);
    summary.last_seq = Some(watermark);
    Ok((StatusCode::CREATED, Json(summary)))
}

/// `DELETE /v1/threads?workspace=…` — bulk delete for one project group.
///
/// The rail's per-project delete action removes every **settled** session of
/// that project in one call; threads with a running turn are reported as
/// skipped (`conflict`) and stay, matching the single-thread route's refusal.
/// `workspace` is required: an empty value would silently mean "the default
/// group", which this verb deliberately does not serve.
pub async fn remove_by_workspace(
    State(state): State<AppState>,
    Query(query): Query<ThreadWorkspaceQuery>,
) -> Result<Json<BulkDeleteThreadsResponse>, ApiError> {
    let path = query.workspace.as_deref().map(str::trim).unwrap_or("");
    if path.is_empty() {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            "workspace query parameter is required",
        ));
    }
    // Serializes against turn registration and single deletes, the same lock
    // the single-thread delete takes: a thread cannot be registered mid-sweep.
    let _workspace_guard = state.workspace_gate.lock().await;
    let response = crate::service::threads::delete_settled_by_workspace(&state, path).await?;
    Ok(Json(response))
}

/// Parse a thread id, reporting a validation error instead of a 500.
pub fn parse_thread(raw: &str) -> Result<ThreadId, ApiError> {
    raw.parse::<ThreadId>().map_err(|err| {
        ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            format!("`{raw}` is not a valid thread id: {err}"),
        )
    })
}

/// Keeps the newest distinct child completion cards within the bounded detail response.
fn recent_child_completions(events: Vec<RuntimeEvent>) -> Vec<ChildCompletionNotice> {
    let mut notification_ids = HashSet::new();
    let mut notices: Vec<_> = events
        .into_iter()
        .filter_map(|event| event.child_completion_notice())
        .rev()
        .filter(|notice| notification_ids.insert(notice.notification_id.clone()))
        .take(MAX_CHILD_COMPLETION_NOTICES)
        .collect();
    notices.reverse();
    notices
}

/// Builds the refresh snapshot from durable lifecycle events, then intersects
/// it with the live process-owned waiters. Durable history supplies display
/// fields; the waiter snapshot is the authorization boundary.
fn project_runtime_snapshot(
    active_turn: Option<ark_contract::TurnSummary>,
    events: Vec<RuntimeEvent>,
    live_approval_ids: impl IntoIterator<Item = impl AsRef<str>>,
    live_input_ids: impl IntoIterator<Item = impl AsRef<str>>,
) -> ThreadRuntimeSnapshot {
    let Some(active_turn) = active_turn else {
        return ThreadRuntimeSnapshot::default();
    };
    let live_approvals: HashSet<String> = live_approval_ids
        .into_iter()
        .map(|id| id.as_ref().to_string())
        .collect();
    let live_inputs: HashSet<String> = live_input_ids
        .into_iter()
        .map(|id| id.as_ref().to_string())
        .collect();
    let mut approvals = std::collections::HashMap::new();
    let mut inputs = std::collections::HashMap::new();
    for event in events {
        match event {
            RuntimeEvent::ApprovalRequested {
                approval_id,
                turn_id,
                tool,
                summary,
                timestamp,
                ..
            } if turn_id == active_turn.turn_id => {
                approvals.insert(
                    approval_id,
                    PendingApprovalView {
                        approval_id: String::new(),
                        turn_id,
                        tool,
                        summary,
                        requested_at: timestamp,
                    },
                );
            }
            RuntimeEvent::ApprovalResolved { approval_id, .. } => {
                approvals.remove(&approval_id);
            }
            RuntimeEvent::UserInputRequested {
                request_id,
                turn_id,
                prompt,
                timestamp,
                ..
            } if turn_id == active_turn.turn_id => {
                inputs.insert(
                    request_id,
                    PendingUserInputView {
                        request_id: String::new(),
                        turn_id,
                        prompt,
                        requested_at: timestamp,
                    },
                );
            }
            RuntimeEvent::UserInputResolved { request_id, .. } => {
                inputs.remove(&request_id);
            }
            _ => {}
        }
    }
    let mut pending_approvals: Vec<_> = approvals
        .into_iter()
        .filter_map(|(approval_id, mut view)| {
            live_approvals.contains(&approval_id).then(|| {
                view.approval_id = approval_id;
                view
            })
        })
        .collect();
    pending_approvals.sort_by_key(|view| view.requested_at);
    let mut pending_user_inputs: Vec<_> = inputs
        .into_iter()
        .filter_map(|(request_id, mut view)| {
            live_inputs.contains(&request_id).then(|| {
                view.request_id = request_id;
                view
            })
        })
        .collect();
    pending_user_inputs.sort_by_key(|view| view.requested_at);
    ThreadRuntimeSnapshot {
        active_turn: Some(active_turn),
        pending_approvals,
        pending_user_inputs,
    }
}

#[cfg(test)]
mod tests;
