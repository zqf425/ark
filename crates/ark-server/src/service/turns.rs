//! Turn-start orchestration (design §7.9).
//!
//! `POST /v1/threads/{id}/turns`, the review route and the scheduler all start
//! turns, so the semantics (size limit, one-turn-per-thread conflict, attach
//! authorization, 202-style registration) live here once. This module returns
//! domain types only: no `StatusCode`, no `Json`, no `ApiError` — the HTTP
//! layer decides how a [`TurnSummary`] or an [`ArkError`] becomes a response.

use std::sync::Arc;

use ark_agent::CancelToken;
use ark_common::{ThreadId, TurnId};
use ark_contract::{TurnSummary, MAX_TURN_TEXT_BYTES};

use crate::state::{AppState, TurnEnd, TurnJob, TurnSource};

/// Shared turn-start core: validate the text, register the turn, spawn the
/// worker.
///
/// `POST /v1/threads/{id}/turns` and `POST /v1/threads/{id}/review` both go
/// through this, so a review observes exactly the same size limit, unknown
/// thread, and one-turn-per-thread conflict semantics as a normal message.
pub async fn start_turn(
    state: &AppState,
    thread_id: ThreadId,
    raw_text: String,
    requested_attachments: Vec<ark_common::AttachmentId>,
) -> ark_common::Result<TurnSummary> {
    if raw_text.len() > MAX_TURN_TEXT_BYTES {
        return Err(ark_common::ArkError::validation(format!(
            "turn text exceeds the {MAX_TURN_TEXT_BYTES}-byte UTF-8 limit"
        )));
    }
    let text = raw_text.trim().to_string();
    if text.is_empty() {
        return Err(ark_common::ArkError::validation(
            "turn text must not be empty",
        ));
    }
    let turn_id = TurnId::new();
    let cancel = CancelToken::new();
    // Serialize explicit input with auto-continuation eligibility. User intent
    // wins by cancelling a pending debounce before registering this turn.
    let _workspace_guard = state.workspace_gate.lock().await;
    state.store.get_thread(thread_id).await?;
    // Authorize before registering: a rejected attachment must not leave a
    // registered turn behind, and the check compares against this thread's own
    // association events only.
    let attachment_ids =
        crate::service::authorize_turn_attachments(state, thread_id, requested_attachments).await?;
    let Some(registration) = state.turns.try_register(thread_id, turn_id, cancel.clone()) else {
        let running = state
            .turns
            .active_for(thread_id)
            .map(|slot| slot.turn_id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Err(ark_common::ArkError::new(
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
        attachment_ids,
    };
    spawn_turn(state.clone(), job);
    Ok(registration.summary)
}

/// Run the turn in the background and fold the outcome back into the registry.
///
/// Detached on purpose: the HTTP response is the `202`, and the turn's progress
/// reaches the client through the event stream. Detached does **not** mean
/// unsupervised: the outer task owns the worker's `JoinHandle`, so a panic or an
/// aborted task still lands on a terminal state (design §3 / §7.1). Without this,
/// one panicking turn would leave the registry reporting `running` forever and
/// every later turn on that thread would answer 409.
pub fn spawn_turn(state: AppState, job: TurnJob) {
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
            Ok(Err(err)) => TurnEnd::Failed {
                message: err.message.clone(),
                model_request_failed: err.code == ark_common::ArkErrorCode::ModelRequestFailed,
            },
            Err(join) => {
                let message = if join.is_panic() {
                    "turn executor panicked"
                } else {
                    "turn was cancelled before it finished"
                };
                TurnEnd::Failed {
                    message: message.to_string(),
                    model_request_failed: false,
                }
            }
        };
        turns.finish(turn_id, &end);
        if let TurnEnd::Failed { message, .. } = &end {
            // A turn that dies outside the reactor still needs a complete
            // lifecycle record: either the log stops at `turn_started` and a
            // reconnect replays a turn that will never finish, or a terminal
            // record appears for a turn the log never shows starting.
            supervisor_state
                .record_turn_terminal(thread_id, turn_id, is_auto_continuation, &end)
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
            TurnEnd::Failed { .. } | TurnEnd::Aborted(_) if is_auto_continuation => {
                tracing::warn!(
                    turn = %turn_id,
                    "automatic continuation failed or was aborted; awaiting explicit input or a new child completion"
                );
            }
            TurnEnd::Failed { .. } | TurnEnd::Aborted(_) => {}
        }
    });
}
