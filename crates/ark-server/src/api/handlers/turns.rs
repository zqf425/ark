//! Turn routes (design §7.9): start, get, steer, interrupt, compact.
//!
//! Starting a turn registers it, hands the job to the injected [`TurnRunner`]
//! and answers `202` immediately: the client follows progress over SSE, never by
//! long-polling this endpoint.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;

use ark_agent::CancelToken;
use ark_common::{ThreadId, TurnId};
use ark_contract::{RuntimeEvent, TurnRequest, TurnSummary};

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::state::{AppState, TurnEnd, TurnJob};

/// `POST /v1/threads/{id}/turns`.
pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<TurnRequest>,
) -> Result<(StatusCode, Json<TurnSummary>), ApiError> {
    let thread_id = parse_thread(&id)?;
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            "turn text must not be empty",
        ));
    }
    // Fail fast on an unknown thread instead of starting a detached task.
    state.store.get_thread(thread_id).await?;

    let turn_id = TurnId::new();
    let cancel = CancelToken::new();
    // The existing runtime rejects a second turn on a busy thread
    // (`conflictPolicy: 'reject'` -> 409) instead of interleaving two model loops
    // over one event log. The renderer keeps the message queued and retries after
    // the running turn settles.
    let Some(summary) = state.turns.try_register(thread_id, turn_id, cancel.clone()) else {
        let running = state
            .turns
            .active_for(thread_id)
            .map(|slot| slot.turn_id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Err(ApiError::new(
            ark_common::ArkErrorCode::Conflict,
            format!("thread already has a running turn: {running}"),
        ));
    };
    let job = TurnJob {
        thread_id,
        turn_id,
        text,
        cancel,
    };
    spawn_turn(state.clone(), job);
    Ok((StatusCode::ACCEPTED, Json(summary)))
}

/// Run the turn in the background and fold the outcome back into the registry.
///
/// Detached on purpose: the HTTP response is the `202`, and the turn's progress
/// reaches the client through the event stream.
fn spawn_turn(state: AppState, job: TurnJob) {
    let turn_id = job.turn_id;
    let turns = state.turns.clone();
    let runner = state.runner.clone();
    tokio::spawn(async move {
        let end = match runner.run_turn(job).await {
            Ok(end) => end,
            Err(err) => TurnEnd::Failed(err.message.clone()),
        };
        turns.finish(turn_id, &end);
        if matches!(end, TurnEnd::Completed) {
            state.note_turn_completed();
        }
    });
}

/// `GET /v1/threads/{id}/turns/{turn_id}`.
pub async fn get(
    State(state): State<AppState>,
    Path((id, turn)): Path<(String, String)>,
) -> Result<Json<TurnSummary>, ApiError> {
    let _thread_id = parse_thread(&id)?;
    let turn_id = parse_turn(&turn)?;
    state
        .turns
        .get(turn_id)
        .map(Json)
        .ok_or_else(|| ApiError::new(ark_common::ArkErrorCode::NotFound, "turn is not known"))
}

/// `POST /v1/threads/{id}/turns/{turn_id}/steer`.
///
/// Steering appends a `turn_steered` event so the model sees the instruction on
/// its next round; the reactor owns when that round happens.
pub async fn steer(
    State(state): State<AppState>,
    Path((id, turn)): Path<(String, String)>,
    Json(body): Json<TurnRequest>,
) -> Result<(StatusCode, Json<TurnSummary>), ApiError> {
    let thread_id = parse_thread(&id)?;
    let turn_id = parse_turn(&turn)?;
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            "steer text must not be empty",
        ));
    }
    let summary = state
        .turns
        .get(turn_id)
        .ok_or_else(|| ApiError::new(ark_common::ArkErrorCode::NotFound, "turn is not known"))?;
    if summary.ended_at.is_some() {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::Conflict,
            "turn has already finished and cannot be steered",
        ));
    }
    state
        .store
        .append_event(
            thread_id,
            RuntimeEvent::TurnSteered {
                seq: 0,
                thread_id,
                turn_id,
                timestamp: Utc::now(),
                text,
            },
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(summary)))
}

/// `POST /v1/threads/{id}/turns/{turn_id}/interrupt`.
pub async fn interrupt(
    State(state): State<AppState>,
    Path((id, turn)): Path<(String, String)>,
) -> Result<(StatusCode, Json<TurnSummary>), ApiError> {
    let _thread_id = parse_thread(&id)?;
    let turn_id = parse_turn(&turn)?;
    state
        .turns
        .cancel(turn_id)
        .map(|summary| (StatusCode::ACCEPTED, Json(summary)))
        .ok_or_else(|| ApiError::new(ark_common::ArkErrorCode::NotFound, "turn is not known"))
}

/// `POST /v1/threads/{id}/compact`.
///
/// Records an explicit compaction pair. Folding the JSONL log itself is a store
/// responsibility reached through its writer; this route declares the intent and
/// keeps the boundary honest about what it did.
pub async fn compact(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thread_id = parse_thread(&id)?;
    state.store.get_thread(thread_id).await?;
    let turn_id = TurnId::new();
    state
        .store
        .append_event(
            thread_id,
            RuntimeEvent::CompactionStarted {
                seq: 0,
                thread_id,
                turn_id,
                timestamp: Utc::now(),
            },
        )
        .await?;
    let removed = 0u32;
    state
        .store
        .append_event(
            thread_id,
            RuntimeEvent::CompactionCompleted {
                seq: 0,
                thread_id,
                turn_id,
                timestamp: Utc::now(),
                removed_items: removed,
            },
        )
        .await?;
    Ok(Json(serde_json::json!({
        "thread_id": thread_id,
        "turn_id": turn_id,
        "removed_items": removed,
        "note": "manual compaction marker written",
    })))
}

/// Parse a turn id with a validation error instead of a 500.
pub fn parse_turn(raw: &str) -> Result<TurnId, ApiError> {
    raw.parse::<TurnId>().map_err(|err| {
        ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            format!("`{raw}` is not a valid turn id: {err}"),
        )
    })
}

/// Helper used by tests: the thread id a turn summary belongs to.
pub fn thread_of(summary: &TurnSummary) -> ThreadId {
    summary.thread_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_turn_ids_are_validation_errors() {
        assert_eq!(
            parse_turn("nope").unwrap_err().0.code,
            ark_common::ArkErrorCode::ValidationError
        );
    }

    #[test]
    fn valid_turn_ids_roundtrip() {
        let id = TurnId::new();
        assert_eq!(parse_turn(&id.to_string()).unwrap(), id);
    }
}
