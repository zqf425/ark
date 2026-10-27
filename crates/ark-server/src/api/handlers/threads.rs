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
    CreateThreadRequest, RuntimeEvent, ThreadDetail, ThreadSummary, UpdateThreadRequest,
};
use ark_ports::ThreadMeta;

use crate::api::error::ApiError;
use crate::state::AppState;

/// Placeholder title when the client sends none.
pub const DEFAULT_TITLE: &str = "New thread";

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
    let child_completions = state
        .store
        .replay_events(thread_id, 0)
        .await?
        .iter()
        .filter_map(RuntimeEvent::child_completion_notice)
        .collect();
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
