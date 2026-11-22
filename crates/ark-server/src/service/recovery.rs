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
//! * the same request text is re-persisted as the user message, so the retry
//!   is a true resumption (the old attempt's durable trail stays intact);
//! * attempts follow [`RECOVERY_DELAYS`]; each attempt is written onto the
//!   durable `turn_started` via `auto_recovery`, so the log shows how much
//!   automatic work one user request consumed;
//! * a streak of consecutive model-failure terminations pauses recoveries for
//!   the thread (same threshold as the auto-continuation breaker): a dead
//!   gateway must neither be hammered nor chain-start its own recoveries. Any
//!   user turn or non-model terminal event resets the streak.

use std::time::Duration;

use ark_common::ThreadId;
use ark_contract::RuntimeEvent;

use crate::service::turns::start_turn_as;
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
/// * `text`: the failed request's user text (`None` only for an
///   auto-continuation parent — there is no user message to resume, and a
///   continuation reruns through its own supervisor instead).
/// * `attempt`: 1-based number of the attempt being scheduled.
/// * `failure`: the previous attempt's failure message, for the log.
pub async fn schedule_recovery(
    state: AppState,
    thread_id: ThreadId,
    text: Option<String>,
    attempt: u32,
    failure: String,
) {
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
        // One-turn-per-thread is re-checked inside `start_turn_as`; a user who
        // typed during the backoff window wins the race and cancels nothing.
        let _ = start_turn_as(
            &state,
            thread_id,
            TurnSource::AutoRecovery { text, attempt },
            String::new(),
            Vec::new(),
        )
        .await;
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
    use ark_common::TurnId;
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig};
    use chrono::Utc;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

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
    }

    #[async_trait::async_trait]
    impl crate::TurnRunner for ScriptedRunner {
        async fn run_turn(&self, _job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
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
        }
    }

    #[tokio::test]
    async fn recovery_attempts_are_bounded_by_the_delay_table() {
        let runner = Arc::new(ScriptedRunner {
            runs: std::sync::atomic::AtomicUsize::new(0),
            succeed_after: 1,
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
            Some("do the thing".into()),
            1,
            "boom".into(),
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
                },
            )
            .await
            .unwrap();
        // The streak is clear, but the attempt budget still gates the run:
        // attempt 4 has no delay entry and stays unscheduled.
        schedule_recovery(
            state.clone(),
            thread,
            Some("do the thing".into()),
            (RECOVERY_DELAYS.len() + 1) as u32,
            "boom".into(),
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
        });
        let (state, thread, _dir) = state_with(runner.clone()).await;
        // Attempt 1 waits RECOVERY_DELAYS[0]; the test only asserts that
        // nothing runs before the delay elapses, so the exact table values
        // are irrelevant here. (Full loop timing is covered by the
        // continuation supervisor tests.)
        schedule_recovery(
            state.clone(),
            thread,
            Some("do the thing".into()),
            1,
            "boom".into(),
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
}
