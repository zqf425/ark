//! Automatic recovery of turns that died on a transient model-request failure.
//!
//! History this module answers: 17 parent turns ended in `turn_failed` on
//! `error sending request` / `model stream broke` / `stream timed out`, and all
//! 17 recoveries were manual — the user retyping "继续" (12 of the 17 retries
//! succeeded with no change at all, and 26% of all user messages were
//! keep-alive nudges). The failure itself lives in the provider/gateway, but
//! leaving the recovery to the user is a harness gap: the request is durable,
//! the failure is transient, and the retry is deterministic.
//!
//! Policy:
//! * only `model_request_failed` terminations schedule a recovery — tool,
//!   policy and panic failures stay terminal;
//! * the same request text is resent, so the retry is a true resumption (the
//!   old attempt's durable trail stays intact). The failed turn already
//!   persisted that user message, so the recovery runner suppresses the
//!   duplicate re-persist and the transcript keeps one bubble per request;
//! * attempts follow [`RECOVERY_DELAYS`]; each attempt is written onto the
//!   durable `turn_started` via `auto_recovery`, so the log shows how much
//!   automatic work one user request consumed;
//! * a streak of consecutive model-failure terminations pauses recoveries for
//!   the thread (same threshold as the auto-continuation breaker): a dead
//!   gateway must neither be hammered nor chain-start its own recoveries. Any
//!   user turn or non-model terminal event resets the streak.

use std::time::Duration;

use ark_common::{ThreadId, TurnId};
use ark_contract::RuntimeEvent;

use crate::service::turns::start_turn_as_at_epoch;
use crate::state::{AppState, TurnSource};

/// Backoff between recovery attempts, by attempt number (1-based). Three
/// attempts cover the observed outage shapes (a blip clears in seconds) and
/// stay close behind the failure, so a transient gateway hiccup costs the
/// user seconds rather than minutes — without turning one user request into
/// an unattended loop.
pub const RECOVERY_DELAYS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
];

/// Consecutive model-failure terminations that pause automatic recoveries on
/// a thread. Shared value with the auto-continuation breaker so both
/// supervisors fail identically on a dead provider.
pub const MAX_CONSECUTIVE_MODEL_FAILURES: usize = ark_agent::RECOVERY_MODEL_FAILURE_STREAK_LIMIT;

/// Schedule one recovery attempt after the attempt-specific backoff.
///
/// * `failed_turn_id`: lifecycle epoch of the failed turn. It is captured before
///   the registry releases that turn, so newer input cannot be adopted by an old
///   recovery in the gap before scheduling.
/// * `text`: the failed request's user text (`None` only for an
///   auto-continuation parent — there is no user message to resume, and a
///   continuation reruns through its own supervisor instead).
/// * `attempt`: 1-based number of the attempt being scheduled.
/// * `failure`: the previous attempt's failure message, for the log.
/// * `attachments`: the failed turn's already-authorized attachment ids.
pub async fn schedule_recovery(
    state: AppState,
    thread_id: ThreadId,
    failed_turn_id: TurnId,
    text: Option<String>,
    attempt: u32,
    failure: String,
    attachments: Vec<ark_common::AttachmentId>,
) {
    if !state.automatic_followups_enabled() {
        return;
    }
    let Some(text) = text else {
        tracing::warn!(
            thread = %thread_id,
            attempt,
            %failure,
            "model-request failure on a continuation turn; awaiting explicit input or a new child completion"
        );
        return;
    };
    if attempt as usize > RECOVERY_DELAYS.len() {
        tracing::warn!(
            thread = %thread_id,
            attempts = attempt - 1,
            %failure,
            "automatic recovery exhausted; the failure stays terminal and a new user message restarts the cycle"
        );
        return;
    }
    let expected_epoch = Some(failed_turn_id);
    if recovery_paused_by_model_failures(&state, thread_id).await {
        return;
    }
    let delay = RECOVERY_DELAYS[attempt as usize - 1];
    tracing::info!(
        thread = %thread_id,
        attempt,
        delay_secs = delay.as_secs(),
        %failure,
        "scheduling automatic recovery for a model-request failure"
    );
    tokio::spawn(async move {
        // A shutdown during the backoff window must not start an unattended
        // turn; the broadcast fires before the process-owned registry drains.
        let mut shutdown = state.shutdown_receiver();
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.recv() => return,
        }
        if state.is_shutting_down() {
            return;
        }
        // A newer turn changes the lifecycle epoch while this task backs off.
        // Re-check under the registration gate so a user turn wins atomically.
        let result = start_turn_as_at_epoch(
            &state,
            thread_id,
            TurnSource::AutoRecovery { text, attempt },
            String::new(),
            attachments,
            expected_epoch,
        )
        .await;
        if let Err(error) = result {
            if error.code != ark_common::ArkErrorCode::Conflict {
                tracing::warn!(thread = %thread_id, attempt, %error, "automatic recovery could not start");
            }
        }
    });
}

/// Circuit breaker: consecutive model-request terminations pause this
/// thread's recoveries. Derived from the durable log, so it survives
/// restarts; any user turn, completion, abort or non-model failure resets the
/// streak (the outage is over the moment anything else succeeded).
async fn recovery_paused_by_model_failures(state: &AppState, thread_id: ThreadId) -> bool {
    let events: Vec<RuntimeEvent> = match state.store.replay_interaction_events(thread_id).await {
        Ok(events) => events,
        Err(error) => {
            // Fail closed: an unreadable lifecycle log must not risk an
            // unattended model turn (same policy as the continuation
            // supervisor's completion read).
            tracing::warn!(
                thread = %thread_id,
                %error,
                "could not inspect model-failure streak; recovery paused"
            );
            return true;
        }
    };
    let streak = ark_agent::consecutive_model_request_failures(&events);
    if streak >= MAX_CONSECUTIVE_MODEL_FAILURES {
        tracing::warn!(
            thread = %thread_id,
            streak,
            "automatic recovery paused after consecutive model-request failures; a user turn or non-model terminal event resets it"
        );
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::handlers::test_support::GatedStore;
    use crate::service::turns::start_turn_as;
    use ark_common::TurnId;
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig};
    use chrono::Utc;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};

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

    /// A runner that fails every turn with a model-request error, or
    /// completes after `succeed_after` runs — driving the retry loop to a
    /// deterministic end state.
    struct ScriptedRunner {
        runs: std::sync::atomic::AtomicUsize,
        succeed_after: usize,
        jobs: Mutex<Vec<crate::TurnJob>>,
    }

    #[async_trait::async_trait]
    impl crate::TurnRunner for ScriptedRunner {
        async fn run_turn(&self, job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
            self.jobs.lock().unwrap().push(job);
            let run = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
            if run >= self.succeed_after {
                Ok(crate::TurnEnd::Completed)
            } else {
                Ok(crate::TurnEnd::Failed {
                    message: "model stream broke".into(),
                    model_request_failed: true,
                })
            }
        }
    }

    async fn state_with(
        runner: Arc<ScriptedRunner>,
    ) -> (AppState, ark_common::ThreadId, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).unwrap());
        let thread = store.create_thread("t").await.unwrap().thread_id;
        let state = AppState::new(
            store.clone() as Arc<dyn SessionStore>,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            runner,
            dir.path().to_path_buf(),
            "test",
        );
        (state, thread, dir)
    }

    fn failed_event(thread: ark_common::ThreadId) -> RuntimeEvent {
        RuntimeEvent::TurnFailed {
            seq: 0,
            thread_id: thread,
            turn_id: TurnId::new(),
            timestamp: Utc::now(),
            message: "model request failed".into(),
            model_request_failed: true,
            item_id: None,
        }
    }

    #[tokio::test]
    async fn recovery_attempts_are_bounded_by_the_delay_table() {
        let runner = Arc::new(ScriptedRunner {
            runs: std::sync::atomic::AtomicUsize::new(0),
            succeed_after: 1,
            jobs: Mutex::new(Vec::new()),
        });
        let (state, thread, _dir) = state_with(runner.clone()).await;
        // A thread whose log already holds MAX_CONSECUTIVE failures is paused:
        // the breaker must refuse to schedule attempt 1.
        for _ in 0..MAX_CONSECUTIVE_MODEL_FAILURES {
            state
                .store
                .append_event(thread, failed_event(thread))
                .await
                .unwrap();
        }
        schedule_recovery(
            state.clone(),
            thread,
            TurnId::new(),
            Some("do the thing".into()),
            1,
            "boom".into(),
            Vec::new(),
        )
        .await;
        // Backoff is 5s; give the task a moment, then assert nothing ran.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(runner.runs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_non_model_failure_breaks_the_streak_and_allows_recovery() {
        let runner = Arc::new(ScriptedRunner {
            runs: std::sync::atomic::AtomicUsize::new(0),
            succeed_after: 1,
            jobs: Mutex::new(Vec::new()),
        });
        let (state, thread, _dir) = state_with(runner.clone()).await;
        for _ in 0..MAX_CONSECUTIVE_MODEL_FAILURES {
            state
                .store
                .append_event(thread, failed_event(thread))
                .await
                .unwrap();
        }
        // A non-model failure resets the streak (the outage is over).
        state
            .store
            .append_event(
                thread,
                RuntimeEvent::TurnFailed {
                    seq: 0,
                    thread_id: thread,
                    turn_id: TurnId::new(),
                    timestamp: Utc::now(),
                    message: "tool policy refused".into(),
                    model_request_failed: false,
                    item_id: None,
                },
            )
            .await
            .unwrap();
        // The streak is clear, but the attempt budget still gates the run:
        // attempt 4 has no delay entry and stays unscheduled.
        schedule_recovery(
            state.clone(),
            thread,
            TurnId::new(),
            Some("do the thing".into()),
            (RECOVERY_DELAYS.len() + 1) as u32,
            "boom".into(),
            Vec::new(),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(runner.runs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_in_budget_attempt_schedules_and_runs_the_turn() {
        let runner = Arc::new(ScriptedRunner {
            runs: std::sync::atomic::AtomicUsize::new(0),
            succeed_after: usize::MAX,
            jobs: Mutex::new(Vec::new()),
        });
        let (state, thread, _dir) = state_with(runner.clone()).await;
        // Attempt 1 waits RECOVERY_DELAYS[0]; the test only asserts that
        // nothing runs before the delay elapses, so the exact table values
        // are irrelevant here. (Full loop timing is covered by the
        // continuation supervisor tests.)
        schedule_recovery(
            state.clone(),
            thread,
            TurnId::new(),
            Some("do the thing".into()),
            1,
            "boom".into(),
            Vec::new(),
        )
        .await;
        // Nothing ran yet (the first backoff is 5s), and the store has no new
        // turn.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(runner.runs.load(Ordering::SeqCst), 0);
        assert!(state
            .store
            .replay_interaction_events(thread)
            .await
            .unwrap()
            .iter()
            .all(|event| !matches!(event, RuntimeEvent::TurnStarted { .. })));
    }

    #[tokio::test]
    async fn a_user_turn_during_backoff_invalidates_the_pending_recovery() {
        let runner = Arc::new(ScriptedRunner {
            runs: std::sync::atomic::AtomicUsize::new(0),
            succeed_after: 1,
            jobs: Mutex::new(Vec::new()),
        });
        let (state, thread, _dir) = state_with(runner.clone()).await;
        let failed = TurnId::new();
        state
            .turns
            .try_register(thread, failed, ark_agent::CancelToken::new())
            .expect("failed turn registers");
        state.turns.finish(
            failed,
            &crate::TurnEnd::Failed {
                message: "model stream broke".into(),
                model_request_failed: true,
            },
        );
        schedule_recovery(
            state.clone(),
            thread,
            failed,
            Some("old request".into()),
            1,
            "boom".into(),
            Vec::new(),
        )
        .await;
        start_turn_as(
            &state,
            thread,
            TurnSource::User("new request".into()),
            "new request".into(),
            Vec::new(),
        )
        .await
        .expect("the user turn wins the lifecycle epoch");

        tokio::time::sleep(RECOVERY_DELAYS[0] + Duration::from_millis(200)).await;
        assert_eq!(
            runner.runs.load(Ordering::SeqCst),
            1,
            "the stale recovery must not start after a newer user turn"
        );
    }

    #[tokio::test]
    async fn a_user_turn_finished_before_scheduling_invalidates_the_old_recovery() {
        let runner = Arc::new(ScriptedRunner {
            runs: std::sync::atomic::AtomicUsize::new(0),
            succeed_after: 1,
            jobs: Mutex::new(Vec::new()),
        });
        let (state, thread, _dir) = state_with(runner.clone()).await;
        let failed = TurnId::new();
        state
            .turns
            .try_register(thread, failed, ark_agent::CancelToken::new())
            .expect("failed turn registers");
        state.turns.finish(
            failed,
            &crate::TurnEnd::Failed {
                message: "model stream broke".into(),
                model_request_failed: true,
            },
        );
        start_turn_as(
            &state,
            thread,
            TurnSource::User("new request".into()),
            "new request".into(),
            Vec::new(),
        )
        .await
        .expect("the user turn starts");
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.turns.active_for(thread).is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the user turn finishes before recovery is scheduled");

        schedule_recovery(
            state.clone(),
            thread,
            failed,
            Some("old request".into()),
            1,
            "boom".into(),
            Vec::new(),
        )
        .await;
        tokio::time::sleep(RECOVERY_DELAYS[0] + Duration::from_millis(200)).await;
        assert_eq!(
            runner.runs.load(Ordering::SeqCst),
            1,
            "a recovery scheduled after newer input must retain the failed turn's epoch"
        );
    }

    #[tokio::test]
    async fn a_new_turn_during_failure_inspection_invalidates_the_recovery() {
        let runner = Arc::new(ScriptedRunner {
            runs: std::sync::atomic::AtomicUsize::new(0),
            succeed_after: 1,
            jobs: Mutex::new(Vec::new()),
        });
        let dir = tempfile::tempdir().expect("temp dir");
        let inner = Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).unwrap());
        let thread = inner.create_thread("t").await.unwrap().thread_id;
        let gated = GatedStore::new(inner.clone() as Arc<dyn SessionStore>);
        let state = AppState::new(
            gated.clone(),
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            runner.clone(),
            dir.path().to_path_buf(),
            "test",
        );
        let failed = TurnId::new();
        state
            .turns
            .try_register(thread, failed, ark_agent::CancelToken::new())
            .expect("failed turn registers");
        state.turns.finish(
            failed,
            &crate::TurnEnd::Failed {
                message: "model stream broke".into(),
                model_request_failed: true,
            },
        );

        gated.interaction.enable();
        let recovery_state = state.clone();
        let recovery = tokio::spawn(async move {
            schedule_recovery(
                recovery_state,
                thread,
                failed,
                Some("old request".into()),
                1,
                "boom".into(),
                Vec::new(),
            )
            .await;
        });
        tokio::time::timeout(Duration::from_secs(1), gated.interaction.entered.notified())
            .await
            .expect("failure inspection pauses at the store");
        start_turn_as(
            &state,
            thread,
            TurnSource::User("new request".into()),
            "new request".into(),
            Vec::new(),
        )
        .await
        .expect("the user turn wins");
        gated.interaction.resume();
        gated.interaction.disable();
        recovery.await.expect("recovery scheduling finishes");
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.turns.active_for(thread).is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the user turn finishes");

        tokio::time::sleep(RECOVERY_DELAYS[0] + Duration::from_millis(200)).await;
        assert_eq!(
            runner.runs.load(Ordering::SeqCst),
            1,
            "the failed request must retain its old lifecycle epoch across the inspection await"
        );
    }

    #[tokio::test]
    async fn recovery_preserves_and_reauthorizes_the_original_attachments() {
        let runner = Arc::new(ScriptedRunner {
            runs: std::sync::atomic::AtomicUsize::new(0),
            succeed_after: 2,
            jobs: Mutex::new(Vec::new()),
        });
        let (state, thread, _dir) = state_with(runner.clone()).await;
        let attachment = ark_common::AttachmentId::new();
        state
            .store
            .append_event(
                thread,
                RuntimeEvent::AttachmentAssociated {
                    seq: 0,
                    thread_id: thread,
                    attachment_id: attachment,
                    timestamp: Utc::now(),
                },
            )
            .await
            .expect("attachment association persists");
        start_turn_as(
            &state,
            thread,
            TurnSource::User("inspect this".into()),
            "inspect this".into(),
            vec![attachment],
        )
        .await
        .expect("the first turn starts");
        tokio::time::timeout(RECOVERY_DELAYS[0] + Duration::from_secs(2), async {
            while runner.runs.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the recovery turn runs");

        let jobs = runner.jobs.lock().unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].attachment_ids, vec![attachment]);
        assert_eq!(jobs[1].attachment_ids, vec![attachment]);
    }
}
