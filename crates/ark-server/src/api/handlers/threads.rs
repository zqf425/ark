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
    ChildCompletionNotice, CreateThreadRequest, PendingApprovalView, PendingUserInputView,
    RuntimeEvent, ThreadDetail, ThreadListQuery, ThreadRuntimeSnapshot, ThreadSummary,
    UpdateThreadRequest, THREAD_PAGE_DEFAULT_LIMIT, THREAD_PAGE_MAX_LIMIT,
};
use ark_ports::ThreadMeta;
use std::collections::HashSet;

use crate::api::error::ApiError;
use crate::state::AppState;

/// Placeholder title when the client sends none.
pub const DEFAULT_TITLE: &str = "New thread";
const MAX_CHILD_COMPLETION_NOTICES: usize = 50;

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
    let limit = query
        .limit
        .unwrap_or(THREAD_PAGE_DEFAULT_LIMIT)
        .clamp(1, THREAD_PAGE_MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);
    let needle = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());

    let metas = state.store.list_threads().await?;
    // Deterministic filter-then-page: sorting here (instead of trusting index
    // order alone) keeps pagination stable across calls, because the tiebreak
    // is the immutable id.
    let mut rows: Vec<&ark_ports::ThreadMeta> = metas
        .iter()
        .filter(|meta| query.archived.is_none_or(|want| meta.archived == want))
        .filter(|meta| needle.is_none_or(|n| meta.title.to_lowercase().contains(&n.to_lowercase())))
        .collect();
    rows.sort_by(|a, b| {
        a.archived
            .cmp(&b.archived)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.thread_id.cmp(&b.thread_id))
    });

    let summaries = rows
        .iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|meta| summary_of(meta))
        .collect();
    Ok(Json(summaries))
}

/// `POST /v1/threads`.
pub async fn create(
    State(state): State<AppState>,
    body: Option<Json<CreateThreadRequest>>,
) -> Result<(StatusCode, Json<ThreadSummary>), ApiError> {
    let title = body
        .and_then(|Json(request)| request.title)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_TITLE.to_string());
    let meta = state.store.create_thread(&title).await?;
    // Publish the creation so an SSE client attached to the new thread sees it
    // without a special case in the stream handler.
    let event = RuntimeEvent::ThreadCreated {
        seq: 0,
        thread_id: meta.thread_id,
        timestamp: Utc::now(),
        title: meta.title.clone(),
    };
    state.store.append_event(meta.thread_id, event).await?;
    let mut summary = summary_of(&meta);
    summary.last_seq = state.store.highest_seq(meta.thread_id).await.ok();
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
    let child_ids = state
        .store
        .list_child_runs(thread_id)
        .await
        .map(|records| records.into_iter().map(|record| record.child_id).collect())
        .unwrap_or_default();
    let mut summary = summary_of(&meta);
    summary.last_seq = state.store.highest_seq(thread_id).await.ok();
    Ok(ThreadDetail {
        summary,
        items,
        child_ids,
        child_completions,
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
        if !matches!(value, "strict" | "workspace_write" | "unattended") {
            return Err(ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                "invalid autonomy",
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
    state.continuations.allow(thread_id);
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

/// Parse a thread id, reporting a validation error instead of a 500.
pub fn parse_thread(raw: &str) -> Result<ThreadId, ApiError> {
    raw.parse::<ThreadId>().map_err(|err| {
        ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            format!("`{raw}` is not a valid thread id: {err}"),
        )
    })
}

/// Map stored metadata onto the wire shape.
pub fn summary_of(meta: &ThreadMeta) -> ThreadSummary {
    ThreadSummary {
        thread_id: meta.thread_id,
        title: meta.title.clone(),
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        last_seq: None,
        archived: meta.archived,
    }
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
