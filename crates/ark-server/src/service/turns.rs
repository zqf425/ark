//! Turn-start orchestration (design §7.9).
//!
//! `POST /v1/threads/{id}/turns`, the review route, the scheduler and the IM
//! service all start turns, so the semantics (size limit, one-turn-per-thread
//! conflict, attach authorization, 202-style registration) live here once. This
//! module returns domain types only: no `StatusCode`, no `Json`, no `ApiError`
//! — the HTTP layer decides how a [`TurnSummary`] or an [`ArkError`] becomes a
//! response.

use std::sync::Arc;

use ark_agent::CancelToken;
use ark_common::{ThreadId, TurnId};
use ark_contract::{TurnSummary, MAX_TURN_TEXT_BYTES};
use ark_ports::{HookScope, TurnOutcomeFact};

use crate::state::{AppState, TurnEnd, TurnJob, TurnSource};

/// Shared turn-start core: validate the text, register the turn, spawn the
/// worker, with the source recorded as explicit user input.
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
    start_turn_as(
        state,
        thread_id,
        TurnSource::User(raw_text.clone()),
        raw_text,
        requested_attachments,
    )
    .await
}

/// [`start_turn`] with an explicit [`TurnSource`]: the IM service records the
/// channel/chat pair on the turn while the text still lands as a persisted user
/// message (via `turn_source_texts` in the composition root).
///
/// `source` carries its own copy of the text; the validation and seed text both
/// come from `raw_text`, so a mismatch between the two is a caller bug — that
/// is why this function is `pub(crate)`-shaped (exported for the IM service in
/// the same crate) rather than a second HTTP entry point.
pub async fn start_turn_as(
    state: &AppState,
    thread_id: ThreadId,
    source: TurnSource,
    raw_text: String,
    requested_attachments: Vec<ark_common::AttachmentId>,
) -> ark_common::Result<TurnSummary> {
    if raw_text.len() > MAX_TURN_TEXT_BYTES {
        return Err(ark_common::ArkError::validation(format!(
            "turn text exceeds the {MAX_TURN_TEXT_BYTES}-byte UTF-8 limit"
        )));
    }
    if raw_text.trim().is_empty() {
        return Err(ark_common::ArkError::validation(
            "turn text must not be empty",
        ));
    }
    let turn_id = TurnId::new();
    let cancel = CancelToken::new();
    // The turn.pre_start hook fires before the workspace gate is acquired:
    // a denial must not leave a half-registered turn behind, and a hook
    // callback that re-entered the host through a gate-protected route would
    // self-deadlock the global gate (requirements/plugin-hook-bus.md,
    // Decide 调用纪律). With NoHooks installed this call is a no-op.
    state
        .hooks
        .on_turn_pre_start(
            HookScope {
                thread_id: &thread_id,
                turn_id: &turn_id,
            },
            turn_source_label(&source),
        )
        .await?;
    // Authorize attachments before taking the global workspace gate: this
    // replays the thread's whole event log, and a slow disk must not stall turn
    // starts for every other thread (AGENTS.md §7.5). A rejected attachment
    // still leaves no registered turn behind, and the check compares against
    // this thread's own association events only.
    let attachment_ids =
        crate::service::authorize_turn_attachments(state, thread_id, requested_attachments).await?;
    // Serialize explicit input with auto-continuation eligibility. User intent
    // wins by cancelling a pending debounce before registering this turn.
    // The gate covers only the registry mutation plus one lightweight thread
    // existence check; that check is shared with the delete route so a
    // concurrent delete cannot register a turn for a thread already gone.
    let _workspace_guard = state.workspace_gate.lock().await;
    state.store.get_thread(thread_id).await?;
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
        source,
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
        // turn.completed/.failed/.aborted observation (requirements/
        // plugin-hook-bus.md): the supervisor is the single turn-level seam, so
        // both registration paths report here and the hook call holds no lock.
        supervisor_state
            .hooks
            .observe_turn_end(
                ark_ports::HookScope {
                    thread_id: &thread_id,
                    turn_id: &turn_id,
                },
                &match &end {
                    TurnEnd::Completed | TurnEnd::CompletedWithPendingCompletions => {
                        TurnOutcomeFact::Completed
                    }
                    TurnEnd::Failed { message, .. } => TurnOutcomeFact::Failed {
                        message: message.clone(),
                    },
                    TurnEnd::Aborted(reason) => TurnOutcomeFact::Aborted {
                        reason: reason.clone(),
                    },
                },
            )
            .await;
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

/// Hook-payload label for the turn source (design: `turn.*` payloads carry
/// metadata only). One shared spelling keeps the continuation path and the
/// explicit-input path from drifting apart.
pub(crate) fn turn_source_label(source: &TurnSource) -> &'static str {
    match source {
        TurnSource::User(_) => "user",
        TurnSource::Im { .. } => "im",
        TurnSource::AutoContinuation => "auto_continuation",
    }
}

#[cfg(test)]
mod tests {
    //! The turn-level hook seams are asserted here because both registration
    //! paths (explicit input and auto-continuation) share them: the pre-start
    /// decision and the supervisor's terminal observation.
    use super::*;
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig};
    use std::sync::Mutex;

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

    struct ImmediateRunner;

    #[async_trait::async_trait]
    impl crate::TurnRunner for ImmediateRunner {
        async fn run_turn(&self, _job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
            Ok(crate::TurnEnd::Completed)
        }
    }

    /// Records every hook call; optionally denies the pre-start.
    struct RecordingBroker {
        deny_pre_start: bool,
        events: Mutex<Vec<String>>,
    }

    impl RecordingBroker {
        fn new(deny_pre_start: bool) -> Arc<Self> {
            Arc::new(Self {
                deny_pre_start,
                events: Mutex::new(Vec::new()),
            })
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ark_ports::HookBroker for RecordingBroker {
        async fn on_turn_pre_start(
            &self,
            _scope: ark_ports::HookScope<'_>,
            source: &str,
        ) -> ark_common::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("pre_start:{source}"));
            if self.deny_pre_start {
                return Err(ark_common::ArkError::validation("hook refused the turn"));
            }
            Ok(())
        }

        async fn on_model_request(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _request: &ark_ports::ModelRequest,
        ) -> ark_common::Result<ark_ports::ModelRequestTuning> {
            Ok(ark_ports::ModelRequestTuning::default())
        }

        async fn on_tool_pre_call(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _tool: &str,
            _args_head: &str,
            _args_truncated: bool,
        ) -> ark_common::Result<ark_ports::ToolPreCallVerdict> {
            Ok(ark_ports::ToolPreCallVerdict::Allow)
        }

        async fn observe_turn_end(
            &self,
            _scope: ark_ports::HookScope<'_>,
            outcome: &ark_ports::TurnOutcomeFact,
        ) {
            self.events
                .lock()
                .unwrap()
                .push(format!("turn_end:{outcome:?}"));
        }

        async fn observe_llm_call(
            &self,
            _scope: ark_ports::HookScope<'_>,
            _fact: &ark_ports::LlmCallFact<'_>,
        ) {
        }
    }

    fn state_with(data_dir: &std::path::Path, broker: Arc<RecordingBroker>) -> AppState {
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"));
        let store: Arc<dyn SessionStore> = sessions;
        AppState::new(
            store,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(ImmediateRunner),
            data_dir.to_path_buf(),
            "test",
        )
        .with_hooks(broker)
    }

    #[tokio::test]
    async fn a_denied_pre_start_registers_no_turn() {
        let dir = tempfile::Builder::new()
            .prefix("ark-hook-deny-")
            .tempdir()
            .expect("temp dir");
        let broker = RecordingBroker::new(true);
        let state = state_with(dir.path(), Arc::clone(&broker));
        let thread = state.store.create_thread("t").await.expect("thread");

        let err = start_turn_as(
            &state,
            thread.thread_id,
            TurnSource::User("hello".to_string()),
            "hello".to_string(),
            Vec::new(),
        )
        .await
        .expect_err("the hook denial must refuse the turn");
        assert!(err.message.contains("hook refused"));
        assert!(
            state.turns.active_for(thread.thread_id).is_none(),
            "a denied turn must leave nothing registered"
        );
        assert_eq!(broker.events(), vec!["pre_start:user".to_string()]);
    }

    #[tokio::test]
    async fn an_allowed_turn_reports_its_outcome_to_the_supervisor_hook() {
        let dir = tempfile::Builder::new()
            .prefix("ark-hook-observe-")
            .tempdir()
            .expect("temp dir");
        let broker = RecordingBroker::new(false);
        let state = state_with(dir.path(), Arc::clone(&broker));
        let thread = state.store.create_thread("t").await.expect("thread");

        let summary = start_turn_as(
            &state,
            thread.thread_id,
            TurnSource::User("hello".to_string()),
            "hello".to_string(),
            Vec::new(),
        )
        .await
        .expect("the hook allows the turn");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while state
                .turns
                .get(summary.turn_id)
                .is_none_or(|slot| slot.ended_at.is_none())
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the supervisor settles the turn");

        assert_eq!(
            broker.events(),
            vec![
                "pre_start:user".to_string(),
                "turn_end:Completed".to_string(),
            ]
        );
    }
}
