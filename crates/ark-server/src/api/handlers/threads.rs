//! Thread routes (design §7.9): list, create, get, update, delete.
//!
//! The store is the system of record (design §7.4), so these handlers only
//! translate DTOs and publish the matching lifecycle event. No thread state is
//! cached here.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;

use ark_common::ThreadId;
use ark_contract::{
    ChildCompletionNotice, CreateThreadRequest, PendingApprovalView, PendingUserInputView,
    RuntimeEvent, ThreadDetail, ThreadRuntimeSnapshot, ThreadSummary, UpdateThreadRequest,
};
use ark_ports::ThreadMeta;
use std::collections::HashSet;

use crate::api::error::ApiError;
use crate::state::AppState;

/// Placeholder title when the client sends none.
pub const DEFAULT_TITLE: &str = "New thread";
const MAX_CHILD_COMPLETION_NOTICES: usize = 50;

/// `GET /v1/threads`.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<ThreadSummary>>, ApiError> {
    let metas = state.store.list_threads().await?;
    let summaries = metas.iter().map(summary_of).collect();
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
    Ok(Json(ThreadDetail {
        summary,
        items,
        child_ids,
        child_completions,
        runtime: project_runtime_snapshot(
            state.turns.active_for(thread_id),
            state.store.replay_interaction_events(thread_id).await?,
            state.gates.pending_approval_ids(),
            state.gates.pending_user_input_ids(),
        ),
    }))
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
        meta = state.store.set_archived(thread_id, archived).await?;
    }
    Ok(Json(summary_of(&meta)))
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
    if let Some(active) = state.turns.active_for(thread_id) {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::Conflict,
            format!(
                "thread has a running turn ({}) and cannot be deleted; interrupt it first",
                active.turn_id
            ),
        ));
    }
    state.store.delete_thread(thread_id).await?;
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
mod tests {
    use super::*;
    use ark_common::{ChildId, TurnId};

    #[test]
    fn invalid_thread_ids_are_validation_errors() {
        let error = parse_thread("not-a-uuid").unwrap_err();
        assert_eq!(error.0.code, ark_common::ArkErrorCode::ValidationError);
    }

    #[test]
    fn valid_thread_ids_roundtrip() {
        let id = ThreadId::new();
        assert_eq!(parse_thread(&id.to_string()).unwrap(), id);
    }

    #[test]
    fn summary_mapping_preserves_title_and_archive_flag() {
        let meta = ThreadMeta {
            thread_id: ThreadId::new(),
            title: "hello".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            archived: true,
        };
        let summary = summary_of(&meta);
        assert_eq!(summary.title, "hello");
        assert!(summary.archived);
        assert_eq!(summary.last_seq, None);
    }

    #[test]
    fn thread_detail_keeps_only_the_latest_distinct_completion_cards() {
        let thread = ThreadId::new();
        let events = (1..=55)
            .map(|seq| RuntimeEvent::ChildCompleted {
                seq,
                thread_id: thread,
                timestamp: Utc::now(),
                notification_id: format!("child:{seq}"),
                child_id: ChildId::new(),
                label: "inspect runtime".to_string(),
                status: "completed".to_string(),
                summary: None,
                error: None,
                usage: None,
                tool_invocations: None,
            })
            .collect();

        let notices = recent_child_completions(events);

        assert_eq!(notices.len(), MAX_CHILD_COMPLETION_NOTICES);
        assert_eq!(notices.first().unwrap().seq, 6);
        assert_eq!(notices.last().unwrap().seq, 55);
    }

    #[test]
    fn interaction_projection_keeps_only_unresolved_live_gate_for_active_turn() {
        let thread = ThreadId::new();
        let active = TurnId::new();
        let finished = TurnId::new();
        let now = Utc::now();
        let runtime = project_runtime_snapshot(
            Some(ark_contract::TurnSummary {
                turn_id: active,
                thread_id: thread,
                state: "running".into(),
                started_at: now,
                ended_at: None,
                error: None,
            }),
            vec![
                RuntimeEvent::ApprovalRequested {
                    seq: 1,
                    thread_id: thread,
                    turn_id: active,
                    timestamp: now,
                    approval_id: "live".into(),
                    tool: "bash".into(),
                    summary: "run command".into(),
                },
                RuntimeEvent::ApprovalRequested {
                    seq: 2,
                    thread_id: thread,
                    turn_id: active,
                    timestamp: now,
                    approval_id: "resolved".into(),
                    tool: "write".into(),
                    summary: "update file".into(),
                },
                RuntimeEvent::ApprovalResolved {
                    seq: 3,
                    thread_id: thread,
                    turn_id: active,
                    timestamp: now,
                    approval_id: "resolved".into(),
                    approved: true,
                },
                RuntimeEvent::UserInputRequested {
                    seq: 4,
                    thread_id: thread,
                    turn_id: finished,
                    timestamp: now,
                    request_id: "old".into(),
                    prompt: "old prompt".into(),
                },
            ],
            ["live"],
            std::iter::empty::<&str>(),
        );

        assert_eq!(runtime.active_turn.unwrap().turn_id, active);
        assert_eq!(runtime.pending_approvals.len(), 1);
        assert_eq!(runtime.pending_approvals[0].approval_id, "live");
        assert!(runtime.pending_user_inputs.is_empty());
    }
}
