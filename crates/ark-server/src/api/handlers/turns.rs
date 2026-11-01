//! Turn routes (design §7.9): start, get, steer, interrupt, compact.
//!
//! Starting a turn registers it, hands the job to the injected [`TurnRunner`]
//! and answers `202` immediately: the client follows progress over SSE, never by
//! long-polling this endpoint.

use ark_common::{ThreadId, TurnId};
use ark_contract::{TurnRequest, TurnSummary};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::state::{AppState, SteerError};

/// `POST /v1/threads/{id}/turns`.
pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<TurnRequest>,
) -> Result<(StatusCode, Json<TurnSummary>), ApiError> {
    let thread_id = parse_thread(&id)?;
    let summary =
        crate::service::turns::start_turn(&state, thread_id, body.text, body.attachment_ids)
            .await?;
    Ok((StatusCode::ACCEPTED, Json(summary)))
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
/// Transport only: existence checks, checkpoint detection and persistence are
/// `service::ensure_checkpoint`'s job; this route declares the intent and keeps
/// the boundary honest about what it did.
pub async fn compact(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thread_id = parse_thread(&id)?;
    match crate::service::ensure_checkpoint(&state.store, thread_id).await? {
        crate::service::CompactionOutcome::Existing(existing) => Ok(Json(
            // Checkpoint items are always written with text; the default only
            // guards the wire contract against a `null` slipping through.
            serde_json::json!({ "thread_id": thread_id, "checkpoint": existing.text.unwrap_or_default() }),
        )),
        crate::service::CompactionOutcome::Created {
            turn_id,
            checkpoint,
        } => Ok(Json(serde_json::json!({
            "thread_id": thread_id,
            "turn_id": turn_id,
            "checkpoint": checkpoint,
        }))),
    }
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
    use crate::service::turns::spawn_turn;
    use crate::state::{TurnEnd, TurnJob, TurnSource};
    use ark_agent::CancelToken;
    use ark_contract::RuntimeEvent;
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

    /// A fresh random data directory, removed when the guard drops.
    ///
    /// `tempfile` names it uniquely, so parallel tests never share a store
    /// directory (and never contend for the same data lock).
    fn test_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("ark-turn-")
            .tempdir()
            .expect("temp dir is creatable")
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
        let dir = test_dir();
        let temp = dir.path().to_path_buf();
        let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
        let state = state(&temp, Arc::clone(&runner));
        let thread = state.store.create_thread("limit").await.expect("thread");
        let text = "x".repeat(1024 * 1024 + 1);

        let error = start(
            State(state),
            Path(thread.thread_id.to_string()),
            Json(TurnRequest {
                text,
                attachment_ids: Vec::new(),
            }),
        )
        .await
        .expect_err("oversized text must be rejected");

        assert_eq!(error.0.code, ark_common::ArkErrorCode::ValidationError);
        assert_eq!(error.0.code.http_status(), StatusCode::BAD_REQUEST.as_u16());
        assert_eq!(runner.0.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn late_interrupt_suppresses_a_pending_continuation() {
        let dir = test_dir();
        let temp = dir.path().to_path_buf();
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
    }

    #[tokio::test]
    async fn stale_interrupt_does_not_suppress_a_newer_turn_epoch() {
        let dir = test_dir();
        let temp = dir.path().to_path_buf();
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
    }

    // ---- continuation lifecycle completeness (D1: a started turn always closes) ----

    /// How the fake worker behaves, i.e. which reactor path it stands in for.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Worker {
        /// The runner fails before the reactor journals `turn_started` — the
        /// model-client or provider-resolution failure path.
        FailBeforeStart,
        /// The reactor journals `turn_started`, then completes normally.
        ReactorCompleted,
    }

    struct LifecycleRunner {
        store: Arc<FileSessionStore>,
        worker: Worker,
    }

    #[async_trait::async_trait]
    impl crate::TurnRunner for LifecycleRunner {
        async fn run_turn(&self, job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
            if self.worker == Worker::FailBeforeStart {
                return Err(ark_common::ArkError::model(
                    "gateway 503 no healthy account",
                ));
            }
            let events = ark_agent::TurnEvents::new(job.thread_id, job.turn_id);
            self.store
                .append_event(job.thread_id, events.turn_started())
                .await?;
            self.store
                .append_event(job.thread_id, events.turn_completed())
                .await?;
            Ok(crate::TurnEnd::Completed)
        }
    }

    /// Count the `turn_started` and terminal records for one turn.
    fn lifecycle_counts(events: &[RuntimeEvent], turn: TurnId) -> (usize, usize) {
        let mut started = 0;
        let mut closed = 0;
        for event in events {
            match event {
                RuntimeEvent::TurnStarted { turn_id, .. } if *turn_id == turn => started += 1,
                RuntimeEvent::TurnCompleted { turn_id, .. }
                | RuntimeEvent::TurnFailed { turn_id, .. }
                | RuntimeEvent::TurnAborted { turn_id, .. }
                    if *turn_id == turn =>
                {
                    closed += 1
                }
                _ => {}
            }
        }
        (started, closed)
    }

    /// Start one auto-continuation turn and wait for its supervisor to settle it.
    async fn run_auto_turn(state: &AppState, thread: ThreadId) -> TurnId {
        let turn = TurnId::new();
        let cancel = CancelToken::new();
        let registration = state
            .turns
            .try_register(thread, turn, cancel.clone())
            .expect("idle thread accepts the continuation");
        spawn_turn(
            state.clone(),
            TurnJob {
                thread_id: thread,
                turn_id: turn,
                source: TurnSource::AutoContinuation,
                cancel,
                steering: registration.steering,
                attachment_ids: Vec::new(),
            },
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while state
                .turns
                .get(turn)
                .is_none_or(|summary| summary.ended_at.is_none())
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the supervisor settles the turn");
        turn
    }

    async fn lifecycle_state(data_dir: &std::path::Path, worker: Worker) -> (AppState, ThreadId) {
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"));
        let store: Arc<dyn SessionStore> = sessions.clone();
        let state = AppState::new(
            store,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(LifecycleRunner {
                store: sessions,
                worker,
            }),
            data_dir.to_path_buf(),
            "test",
        );
        let thread = state.store.create_thread("parent").await.expect("thread");
        (state, thread.thread_id)
    }

    #[tokio::test]
    async fn model_failed_continuation_still_records_started_and_failed() {
        let dir = test_dir();
        let temp = dir.path().to_path_buf();
        let (state, thread) = lifecycle_state(&temp, Worker::FailBeforeStart).await;

        // Two consecutive model failures: the circuit breaker's input, and two
        // continuation turns that must each be auditable in the parent log.
        let first = run_auto_turn(&state, thread).await;
        let second = run_auto_turn(&state, thread).await;

        let events = state
            .store
            .replay_interaction_events(thread)
            .await
            .expect("interaction log");
        for turn in [first, second] {
            assert_eq!(
                lifecycle_counts(&events, turn),
                (1, 1),
                "every started continuation turn needs exactly one start and one terminal record"
            );
        }
        assert!(
            events.iter().any(|event| matches!(
                event,
                RuntimeEvent::TurnStarted {
                    turn_id,
                    auto_continuation: true,
                    ..
                } if *turn_id == first
            )),
            "the synthesized start must keep the automatic-continuation marker"
        );
        let failure = events
            .iter()
            .find_map(|event| match event {
                RuntimeEvent::TurnFailed {
                    turn_id,
                    message,
                    model_request_failed,
                    ..
                } if *turn_id == first => Some((message.clone(), *model_request_failed)),
                _ => None,
            })
            .expect("the failed continuation has a terminal record");
        assert!(
            failure.0.contains("gateway 503"),
            "turn_failed keeps a readable reason: {}",
            failure.0
        );
        assert!(failure.1, "the breaker needs model_request_failed");
        assert_eq!(
            ark_agent::consecutive_auto_model_request_failures(&events),
            2,
            "the breaker counts both failures from the durable log"
        );
    }

    #[tokio::test]
    async fn completed_continuation_records_started_and_completed_once() {
        let dir = test_dir();
        let temp = dir.path().to_path_buf();
        let (state, thread) = lifecycle_state(&temp, Worker::ReactorCompleted).await;

        let turn = run_auto_turn(&state, thread).await;

        // A late supervisor close must not append a second terminal record for a
        // turn the reactor already closed.
        state
            .record_turn_terminal(
                thread,
                turn,
                true,
                &TurnEnd::Failed {
                    message: "late duplicate".into(),
                    model_request_failed: false,
                },
            )
            .await;

        let events = state
            .store
            .replay_interaction_events(thread)
            .await
            .expect("interaction log");
        assert_eq!(
            lifecycle_counts(&events, turn),
            (1, 1),
            "the supervisor must not duplicate a lifecycle the reactor already wrote"
        );
    }
}
