//! Turn routes (design §7.9): start, get, steer, interrupt, compact.
//!
//! Starting a turn registers it, hands the job to the injected [`TurnRunner`]
//! and answers `202` immediately: the client follows progress over SSE, never by
//! long-polling this endpoint.

use std::sync::Arc;

use ark_agent::CancelToken;
use ark_common::{ThreadId, TurnId};
use ark_contract::{RuntimeEvent, TurnRequest, TurnSummary, MAX_TURN_TEXT_BYTES};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::state::{AppState, SteerError, TurnEnd, TurnJob, TurnSource};

/// `POST /v1/threads/{id}/turns`.
pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<TurnRequest>,
) -> Result<(StatusCode, Json<TurnSummary>), ApiError> {
    let thread_id = parse_thread(&id)?;
    start_turn(&state, thread_id, body.text).await
}

/// Shared turn-start core: validate the text, register the turn, spawn the
/// worker.
///
/// `POST /v1/threads/{id}/turns` and `POST /v1/threads/{id}/review` both go
/// through this, so a review observes exactly the same size limit, unknown
/// thread, and one-turn-per-thread conflict semantics as a normal message.
pub(crate) async fn start_turn(
    state: &AppState,
    thread_id: ThreadId,
    raw_text: String,
) -> Result<(StatusCode, Json<TurnSummary>), ApiError> {
    if raw_text.len() > MAX_TURN_TEXT_BYTES {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            format!("turn text exceeds the {MAX_TURN_TEXT_BYTES}-byte UTF-8 limit"),
        ));
    }
    let text = raw_text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            "turn text must not be empty",
        ));
    }
    let turn_id = TurnId::new();
    let cancel = CancelToken::new();
    // Serialize explicit input with auto-continuation eligibility. User intent
    // wins by cancelling a pending debounce before registering this turn.
    let _workspace_guard = state.workspace_gate.lock().await;
    state.store.get_thread(thread_id).await?;
    let Some(registration) = state.turns.try_register(thread_id, turn_id, cancel.clone()) else {
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
    state.continuations.allow(thread_id);
    let job = TurnJob {
        thread_id,
        turn_id,
        source: TurnSource::User(text),
        cancel,
        steering: registration.steering,
    };
    spawn_turn(state.clone(), job);
    Ok((StatusCode::ACCEPTED, Json(registration.summary)))
}

/// Run the turn in the background and fold the outcome back into the registry.
///
/// Detached on purpose: the HTTP response is the `202`, and the turn's progress
/// reaches the client through the event stream. Detached does **not** mean
/// unsupervised: the outer task owns the worker's `JoinHandle`, so a panic or an
/// aborted task still lands on a terminal state (design §3 / §7.1). Without this,
/// one panicking turn would leave the registry reporting `running` forever and
/// every later turn on that thread would answer 409.
pub(crate) fn spawn_turn(state: AppState, job: TurnJob) {
    let turn_id = job.turn_id;
    let thread_id = job.thread_id;
    let is_auto_continuation = matches!(&job.source, TurnSource::AutoContinuation);
    let turns = Arc::clone(&state.turns);
    let runner = Arc::clone(&state.runner);
    let supervisor_state = state.clone();
    tokio::spawn(async move {
        // The worker owns the reactor; the supervisor only observes it.
        let worker = tokio::spawn(async move { runner.run_turn(job).await });
        let end = match worker.await {
            Ok(Ok(end)) => end,
            Ok(Err(err)) => TurnEnd::Failed(err.message.clone()),
            Err(join) => {
                let message = if join.is_panic() {
                    "turn executor panicked"
                } else {
                    "turn was cancelled before it finished"
                };
                TurnEnd::Failed(message.to_string())
            }
        };
        turns.finish(turn_id, &end);
        if let TurnEnd::Failed(message) = &end {
            // A turn that dies outside the reactor still needs a terminal event,
            // otherwise the log stops at `turn_started` and a reconnect replays
            // a turn that will never finish.
            supervisor_state
                .record_turn_terminal(thread_id, turn_id, &end)
                .await;
            tracing::warn!(turn = %turn_id, %message, "turn ended without a terminal event");
        }
        match end {
            TurnEnd::CompletedWithPendingCompletions => {
                supervisor_state.note_turn_completed();
                // The previous batch was fully acknowledged, so another bounded
                // turn can safely drain the next inbox page without retrying the
                // same notification forever.
                crate::service::schedule_auto_continuation(supervisor_state, thread_id).await;
            }
            TurnEnd::Completed => {
                supervisor_state.note_turn_completed();
                // A completion may have arrived while an explicit turn was active.
                // Automatic turns only chain through the explicit pending variant
                // above, which requires every acknowledgement to have succeeded.
                if !is_auto_continuation {
                    crate::service::schedule_auto_continuation(supervisor_state, thread_id).await;
                }
            }
            TurnEnd::Failed(_) | TurnEnd::Aborted(_) if is_auto_continuation => {
                tracing::warn!(
                    turn = %turn_id,
                    "automatic continuation failed or was aborted; awaiting explicit input or a new child completion"
                );
            }
            TurnEnd::Failed(_) | TurnEnd::Aborted(_) => {}
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
/// Steering enters the active reactor's bounded queue. The reactor journals the
/// `turn_steered` event only after it has safely added the instruction to model
/// history at the next round boundary.
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
        .enqueue_steering(thread_id, turn_id, &text)
        .map_err(|error| match error {
            SteerError::NotFound => {
                ApiError::new(ark_common::ArkErrorCode::NotFound, "turn is not known")
            }
            SteerError::Finished => ApiError::new(
                ark_common::ArkErrorCode::Conflict,
                "turn has already finished and cannot be steered",
            ),
            SteerError::QueueFull => ApiError::new(
                ark_common::ArkErrorCode::Conflict,
                "turn steering queue is full; wait for the next model round",
            ),
        })?;
    Ok((StatusCode::ACCEPTED, Json(summary)))
}

/// `POST /v1/threads/{id}/turns/{turn_id}/interrupt`.
pub async fn interrupt(
    State(state): State<AppState>,
    Path((id, turn)): Path<(String, String)>,
) -> Result<(StatusCode, Json<TurnSummary>), ApiError> {
    let thread_id = parse_thread(&id)?;
    let turn_id = parse_turn(&turn)?;
    // Trip the active turn immediately; do not make user cancellation wait for
    // an unrelated workspace operation that currently owns the lifecycle gate.
    let cancelled = state.turns.cancel_active(thread_id, turn_id);
    let _workspace_guard = state.workspace_gate.lock().await;
    // Suppress only while this is still the newest epoch. A newer explicit turn
    // may have registered while we waited for the gate and must take precedence.
    if state.turns.is_latest_for(thread_id, turn_id) {
        state.continuations.suppress(thread_id);
    }
    cancelled
        .map(|summary| (StatusCode::ACCEPTED, Json(summary)))
        .ok_or_else(|| {
            ApiError::new(
                ark_common::ArkErrorCode::NotFound,
                "active turn is not known",
            )
        })
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
    let items = state.store.read_items(thread_id).await?;
    if let Some(existing) = items.iter().find_map(|item| {
        item.text
            .as_deref()
            .filter(|text| text.starts_with("[context checkpoint"))
    }) {
        return Ok(Json(
            serde_json::json!({ "thread_id": thread_id, "checkpoint": existing }),
        ));
    }
    let checkpoint = ark_agent::ContextCheckpoint::from_texts(
        items.iter().filter_map(|item| item.text.as_deref()),
    )
    .render();
    let turn_id = TurnId::new();
    let item = ark_contract::ThreadItem::new(turn_id, ark_contract::item::ItemKind::Notice)
        .with_text(checkpoint.clone())
        .completed();
    state
        .store
        .append_event(
            thread_id,
            RuntimeEvent::ItemCreated {
                seq: 0,
                thread_id,
                turn_id,
                timestamp: Utc::now(),
                item,
            },
        )
        .await?;
    Ok(Json(serde_json::json!({
        "thread_id": thread_id,
        "turn_id": turn_id,
        "checkpoint": checkpoint,
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig};
    use axum::extract::{Path, State};
    use axum::http::StatusCode;

    use super::*;

    struct EmptyTools;

    impl ToolRegistryPort for EmptyTools {
        fn names(&self) -> Vec<String> {
            Vec::new()
        }

        fn schemas(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }

        fn version(&self) -> u64 {
            0
        }
    }

    struct CountingRunner(AtomicUsize);

    #[async_trait::async_trait]
    impl crate::TurnRunner for CountingRunner {
        async fn run_turn(&self, _job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(crate::TurnEnd::Completed)
        }
    }

    fn state(data_dir: &std::path::Path, runner: Arc<CountingRunner>) -> AppState {
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"));
        let store: Arc<dyn SessionStore> = sessions;
        AppState::new(
            store,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            runner,
            data_dir.to_path_buf(),
            "test",
        )
    }

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

    #[tokio::test]
    async fn oversized_turn_text_is_rejected_without_starting_a_turn() {
        let temp = std::env::temp_dir().join(format!("ark-turn-limit-{}", TurnId::new()));
        let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
        let state = state(&temp, Arc::clone(&runner));
        let thread = state.store.create_thread("limit").await.expect("thread");
        let text = "x".repeat(1024 * 1024 + 1);

        let error = start(
            State(state),
            Path(thread.thread_id.to_string()),
            Json(TurnRequest { text }),
        )
        .await
        .expect_err("oversized text must be rejected");

        assert_eq!(error.0.code, ark_common::ArkErrorCode::ValidationError);
        assert_eq!(error.0.code.http_status(), StatusCode::BAD_REQUEST.as_u16());
        assert_eq!(runner.0.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(temp);
    }

    #[tokio::test]
    async fn late_interrupt_suppresses_a_pending_continuation() {
        let temp = std::env::temp_dir().join(format!("ark-late-interrupt-{}", TurnId::new()));
        let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
        let state = state(&temp, runner);
        let thread = state.store.create_thread("parent").await.expect("thread");
        let turn = TurnId::new();
        state
            .turns
            .try_register(thread.thread_id, turn, CancelToken::new())
            .expect("turn registered");
        state.turns.finish(turn, &TurnEnd::Completed);
        state
            .continuations
            .try_schedule(thread.thread_id)
            .expect("pending continuation");

        let error = interrupt(
            State(state.clone()),
            Path((thread.thread_id.to_string(), turn.to_string())),
        )
        .await
        .expect_err("the target turn already ended");

        assert_eq!(error.0.code, ark_common::ArkErrorCode::NotFound);
        assert!(state.continuations.try_schedule(thread.thread_id).is_none());
        let _ = std::fs::remove_dir_all(temp);
    }

    #[tokio::test]
    async fn stale_interrupt_does_not_suppress_a_newer_turn_epoch() {
        let temp = std::env::temp_dir().join(format!("ark-stale-interrupt-{}", TurnId::new()));
        let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
        let state = state(&temp, runner);
        let thread = state.store.create_thread("parent").await.expect("thread");
        let stale = TurnId::new();
        state
            .turns
            .try_register(thread.thread_id, stale, CancelToken::new())
            .expect("first turn registered");
        state.turns.finish(stale, &TurnEnd::Completed);
        let current = TurnId::new();
        state
            .turns
            .try_register(thread.thread_id, current, CancelToken::new())
            .expect("newer turn registered");
        state.turns.finish(current, &TurnEnd::Completed);

        let error = interrupt(
            State(state.clone()),
            Path((thread.thread_id.to_string(), stale.to_string())),
        )
        .await
        .expect_err("the stale target already ended");

        assert_eq!(error.0.code, ark_common::ArkErrorCode::NotFound);
        assert!(state.continuations.try_schedule(thread.thread_id).is_some());
        let _ = std::fs::remove_dir_all(temp);
    }
}
