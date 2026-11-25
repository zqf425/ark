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
    start_turn_as_inner(
        state,
        thread_id,
        source,
        raw_text,
        requested_attachments,
        None,
    )
    .await
}

pub(crate) async fn start_turn_as_at_epoch(
    state: &AppState,
    thread_id: ThreadId,
    source: TurnSource,
    raw_text: String,
    requested_attachments: Vec<ark_common::AttachmentId>,
    expected_epoch: Option<TurnId>,
) -> ark_common::Result<TurnSummary> {
    start_turn_as_inner(
        state,
        thread_id,
        source,
        raw_text,
        requested_attachments,
        expected_epoch,
    )
    .await
}

async fn start_turn_as_inner(
    state: &AppState,
    thread_id: ThreadId,
    source: TurnSource,
    raw_text: String,
    requested_attachments: Vec<ark_common::AttachmentId>,
    expected_epoch: Option<TurnId>,
) -> ark_common::Result<TurnSummary> {
    if !state.automatic_followups_enabled()
        && matches!(
            source,
            TurnSource::AutoContinuation | TurnSource::AutoRecovery { .. }
        )
    {
        return Err(ark_common::ArkError::new(
            ark_common::ArkErrorCode::RuntimeUnavailable,
            "automatic follow-up turns are disabled for this runtime",
        ));
    }
    if expected_epoch.is_some_and(|epoch| state.turns.latest_for(thread_id) != Some(epoch)) {
        return Err(ark_common::ArkError::new(
            ark_common::ArkErrorCode::Conflict,
            "turn lifecycle changed while the automatic follow-up was waiting",
        ));
    }
    let raw_text = source_text(&source, raw_text);
    validate_turn_text(&raw_text)?;
    let turn_id = TurnId::new();
    let cancel = CancelToken::new();
    run_pre_start_hook(state, thread_id, turn_id, &source).await?;
    let attachment_ids =
        crate::service::authorize_turn_attachments(state, thread_id, requested_attachments).await?;
    let registration = register_turn(state, thread_id, turn_id, &cancel, expected_epoch).await?;
    name_user_thread(state, thread_id, &source, &raw_text).await;
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

fn source_text(source: &TurnSource, raw_text: String) -> String {
    match source {
        TurnSource::AutoRecovery { text, .. } | TurnSource::Scheduled(text) => text.clone(),
        _ => raw_text,
    }
}

pub(crate) fn validate_turn_text(raw_text: &str) -> ark_common::Result<()> {
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
    Ok(())
}

async fn run_pre_start_hook(
    state: &AppState,
    thread_id: ThreadId,
    turn_id: TurnId,
    source: &TurnSource,
) -> ark_common::Result<()> {
    // The hook runs before workspace_gate, so re-entry cannot deadlock it and
    // a denial cannot leave a half-registered turn behind.
    state
        .hooks
        .on_turn_pre_start(
            HookScope {
                thread_id: &thread_id,
                turn_id: &turn_id,
            },
            turn_source_label(source),
        )
        .await
}

async fn register_turn(
    state: &AppState,
    thread_id: ThreadId,
    turn_id: TurnId,
    cancel: &CancelToken,
    expected_epoch: Option<TurnId>,
) -> ark_common::Result<crate::state::TurnRegistration> {
    let _workspace_guard = state.workspace_gate.lock().await;
    if expected_epoch.is_some_and(|epoch| state.turns.latest_for(thread_id) != Some(epoch)) {
        return Err(ark_common::ArkError::new(
            ark_common::ArkErrorCode::Conflict,
            "turn lifecycle changed while the automatic follow-up was waiting",
        ));
    }
    if state.is_shutting_down() {
        return Err(ark_common::ArkError::new(
            ark_common::ArkErrorCode::RuntimeUnavailable,
            "runtime is shutting down",
        ));
    }
    state.store.get_thread(thread_id).await?;
    let Some(registration) = state.turns.try_register(thread_id, turn_id, cancel.clone()) else {
        if state.is_shutting_down() {
            return Err(ark_common::ArkError::new(
                ark_common::ArkErrorCode::RuntimeUnavailable,
                "runtime is shutting down",
            ));
        }
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
    Ok(registration)
}

async fn name_user_thread(
    state: &AppState,
    thread_id: ThreadId,
    source: &TurnSource,
    raw_text: &str,
) {
    if matches!(source, TurnSource::User(_)) {
        crate::service::thread_titles::name_thread_on_first_turn(
            state.store.as_ref(),
            thread_id,
            raw_text,
        )
        .await;
    }
}

/// Metadata retained while the detached worker owns the turn job.
struct TurnSupervisor {
    turn_id: TurnId,
    thread_id: ThreadId,
    is_auto_continuation: bool,
    recovery: Option<(String, u32, Vec<ark_common::AttachmentId>)>,
}

impl TurnSupervisor {
    fn from_job(job: &TurnJob) -> Self {
        let recovery = match &job.source {
            TurnSource::AutoRecovery { text, attempt } => {
                Some((text.clone(), *attempt, job.attachment_ids.clone()))
            }
            TurnSource::User(text) | TurnSource::Scheduled(text) | TurnSource::Im { text, .. } => {
                Some((text.clone(), 0, job.attachment_ids.clone()))
            }
            TurnSource::AutoContinuation => None,
        };
        Self {
            turn_id: job.turn_id,
            thread_id: job.thread_id,
            is_auto_continuation: matches!(&job.source, TurnSource::AutoContinuation),
            recovery,
        }
    }
}

/// Run the turn in the background and fold the outcome back into the registry.
pub fn spawn_turn(state: AppState, job: TurnJob) {
    let supervisor = TurnSupervisor::from_job(&job);
    let runner = Arc::clone(&state.runner);
    tokio::spawn(async move {
        let end = run_worker(runner, job).await;
        if let Err(error) = observe_turn_end(&state, &supervisor, &end).await {
            tracing::warn!(
                turn = %supervisor.turn_id,
                code = ?error.code,
                %error,
                "turn terminal persistence failed; leaving the turn unsettled"
            );
            return;
        }
        state.turns.finish(supervisor.turn_id, &end);
        schedule_after_turn(state, supervisor, end).await;
    });
}

async fn run_worker(runner: Arc<dyn crate::state::TurnRunner>, job: TurnJob) -> TurnEnd {
    match tokio::spawn(async move { runner.run_turn(job).await }).await {
        Ok(Ok(end)) => end,
        Ok(Err(error)) => TurnEnd::Failed {
            model_request_failed: error.code == ark_common::ArkErrorCode::ModelRequestFailed,
            message: error.message,
        },
        Err(join) => TurnEnd::Failed {
            message: if join.is_panic() {
                "turn executor panicked"
            } else {
                "turn was cancelled before it finished"
            }
            .to_string(),
            model_request_failed: false,
        },
    }
}

async fn observe_turn_end(
    state: &AppState,
    supervisor: &TurnSupervisor,
    end: &TurnEnd,
) -> ark_common::Result<()> {
    let fact = match end {
        TurnEnd::Completed | TurnEnd::CompletedWithPendingCompletions => TurnOutcomeFact::Completed,
        TurnEnd::Failed { message, .. } => TurnOutcomeFact::Failed {
            message: message.clone(),
        },
        TurnEnd::Aborted(reason) => TurnOutcomeFact::Aborted {
            reason: reason.clone(),
        },
    };
    state
        .hooks
        .observe_turn_end(
            ark_ports::HookScope {
                thread_id: &supervisor.thread_id,
                turn_id: &supervisor.turn_id,
            },
            &fact,
        )
        .await;
    state
        .record_turn_terminal(
            supervisor.thread_id,
            supervisor.turn_id,
            supervisor.is_auto_continuation,
            end,
        )
        .await?;
    if let TurnEnd::Failed { message, .. } = end {
        tracing::warn!(turn = %supervisor.turn_id, %message, "turn ended without a terminal event");
    }
    Ok(())
}

async fn schedule_after_turn(state: AppState, supervisor: TurnSupervisor, end: TurnEnd) {
    if !state.automatic_followups_enabled() {
        if matches!(
            end,
            TurnEnd::Completed | TurnEnd::CompletedWithPendingCompletions
        ) {
            state.note_turn_completed();
        }
        return;
    }
    match end {
        TurnEnd::CompletedWithPendingCompletions => {
            state.note_turn_completed();
            crate::service::schedule_auto_continuation(state, supervisor.thread_id).await;
        }
        TurnEnd::Completed => {
            state.note_turn_completed();
            if !supervisor.is_auto_continuation {
                crate::service::schedule_auto_continuation(state, supervisor.thread_id).await;
            }
        }
        TurnEnd::Failed {
            message,
            model_request_failed: true,
        } => schedule_model_recovery(state, supervisor, message).await,
        TurnEnd::Failed { .. } | TurnEnd::Aborted(_) if supervisor.is_auto_continuation => {
            tracing::warn!(
                turn = %supervisor.turn_id,
                "automatic continuation failed or was aborted; awaiting explicit input or a new child completion"
            );
        }
        TurnEnd::Failed { .. } | TurnEnd::Aborted(_) => {}
    }
}

async fn schedule_model_recovery(state: AppState, supervisor: TurnSupervisor, message: String) {
    if let Some((text, previous, attachments)) = supervisor.recovery {
        let attempt = if previous == 0 { 1 } else { previous + 1 };
        crate::service::schedule_recovery(
            state,
            supervisor.thread_id,
            supervisor.turn_id,
            Some(text),
            attempt,
            message,
            attachments,
        )
        .await;
    } else {
        tracing::warn!(
            thread = %supervisor.thread_id,
            %message,
            "model-request failure on a continuation turn; awaiting explicit input or a new child completion"
        );
    }
}

/// Hook-payload label for the turn source (design: `turn.*` payloads carry
/// metadata only). One shared spelling keeps the continuation path and the
/// explicit-input path from drifting apart.
pub(crate) fn turn_source_label(source: &TurnSource) -> &'static str {
    match source {
        TurnSource::User(_) => "user",
        TurnSource::Scheduled(_) => "scheduled",
        TurnSource::Im { .. } => "im",
        TurnSource::AutoContinuation => "auto_continuation",
        TurnSource::AutoRecovery { .. } => "auto_recovery",
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
        end_entered: Option<Arc<tokio::sync::Notify>>,
        end_release: Option<Arc<tokio::sync::Notify>>,
    }

    impl RecordingBroker {
        fn new(deny_pre_start: bool) -> Arc<Self> {
            Arc::new(Self {
                deny_pre_start,
                events: Mutex::new(Vec::new()),
                end_entered: None,
                end_release: None,
            })
        }

        fn blocking() -> (
            Arc<Self>,
            Arc<tokio::sync::Notify>,
            Arc<tokio::sync::Notify>,
        ) {
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            (
                Arc::new(Self {
                    deny_pre_start: false,
                    events: Mutex::new(Vec::new()),
                    end_entered: Some(Arc::clone(&entered)),
                    end_release: Some(Arc::clone(&release)),
                }),
                entered,
                release,
            )
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
            if let Some(entered) = &self.end_entered {
                entered.notify_one();
            }
            if let Some(release) = &self.end_release {
                release.notified().await;
            }
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
        state_with_store(sessions, broker, data_dir)
    }

    fn state_with_store(
        store: Arc<dyn SessionStore>,
        broker: Arc<RecordingBroker>,
        data_dir: &std::path::Path,
    ) -> AppState {
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
    async fn shutdown_rejects_a_new_turn_without_registering_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state = state_with(dir.path(), RecordingBroker::new(false));
        let thread = state.store.create_thread("t").await.expect("thread");
        state.begin_shutdown();

        let error = start_turn_as(
            &state,
            thread.thread_id,
            TurnSource::User("hello".to_string()),
            "hello".to_string(),
            Vec::new(),
        )
        .await
        .expect_err("shutdown must reject a late turn");

        assert_eq!(error.code, ark_common::ArkErrorCode::RuntimeUnavailable);
        assert!(state.turns.active_for(thread.thread_id).is_none());
        assert!(state
            .turns
            .try_register(thread.thread_id, TurnId::new(), CancelToken::new())
            .is_none());
    }

    #[tokio::test]
    async fn the_terminal_hook_finishes_before_the_turn_slot_is_released() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (broker, entered, release) = RecordingBroker::blocking();
        let state = state_with(dir.path(), Arc::clone(&broker));
        let thread = state.store.create_thread("t").await.expect("thread");
        let first = start_turn_as(
            &state,
            thread.thread_id,
            TurnSource::User("first".to_string()),
            "first".to_string(),
            Vec::new(),
        )
        .await
        .expect("the first turn starts");
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .expect("the terminal hook starts");

        let second = start_turn_as(
            &state,
            thread.thread_id,
            TurnSource::User("second".to_string()),
            "second".to_string(),
            Vec::new(),
        )
        .await;
        release.notify_waiters();

        let error = second.expect_err("the first turn must still own its slot");
        assert_eq!(error.code, ark_common::ArkErrorCode::Conflict);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while state
                .turns
                .get(first.turn_id)
                .is_none_or(|slot| slot.ended_at.is_none())
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the first turn settles after its terminal hook");
    }

    #[tokio::test]
    async fn a_completed_runner_without_journal_events_gets_a_synthetic_lifecycle() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state = state_with(dir.path(), RecordingBroker::new(false));
        let thread = state.store.create_thread("t").await.expect("thread");
        let summary = start_turn_as(
            &state,
            thread.thread_id,
            TurnSource::User("hello".to_string()),
            "hello".to_string(),
            Vec::new(),
        )
        .await
        .expect("the turn starts");
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
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

        let events = state
            .store
            .replay_interaction_events(thread.thread_id)
            .await
            .expect("lifecycle events");
        assert!(events.iter().any(|event| matches!(
            event,
            ark_contract::RuntimeEvent::TurnStarted { turn_id, .. } if *turn_id == summary.turn_id
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ark_contract::RuntimeEvent::TurnCompleted { turn_id, .. } if *turn_id == summary.turn_id
        )));
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

    #[tokio::test]
    async fn failed_terminal_persistence_keeps_the_turn_unsettled() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).expect("session store"));
        let gated = crate::api::handlers::test_support::GatedStore::new(
            sessions.clone() as Arc<dyn SessionStore>
        );
        // The synthesized TurnStarted succeeds; its compensating terminal fails.
        gated.fail_append_at(2);
        let state = state_with_store(gated.clone(), RecordingBroker::new(false), dir.path());
        let thread = state.store.create_thread("t").await.expect("thread");
        let summary = start_turn_as(
            &state,
            thread.thread_id,
            TurnSource::User("hello".to_string()),
            "hello".to_string(),
            Vec::new(),
        )
        .await
        .expect("turn starts");
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            gated.append_failed.notified(),
        )
        .await
        .expect("terminal persistence is attempted");
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }

        assert!(
            state
                .turns
                .get(summary.turn_id)
                .is_some_and(|slot| slot.ended_at.is_none()),
            "a failed durable close must not release the active turn"
        );
        assert_eq!(state.turns_completed(), 0);
        let events = sessions
            .replay_interaction_events(thread.thread_id)
            .await
            .expect("persisted start");
        assert!(events.iter().any(|event| {
            matches!(event, ark_contract::RuntimeEvent::TurnStarted { turn_id, .. }
                if *turn_id == summary.turn_id)
        }));
        assert!(!events.iter().any(|event| {
            matches!(event, ark_contract::RuntimeEvent::TurnCompleted { turn_id, .. }
                if *turn_id == summary.turn_id)
        }));
    }

    /// The first user turn names a still-placeholder thread **before** the
    /// worker runs: when the runner observes the turn, the title is already
    /// durable and the rename event is already in the log ahead of the turn's
    /// own events. A user-renamed thread keeps its name.
    #[tokio::test]
    async fn the_first_user_turn_renames_the_thread_before_the_worker_runs() {
        let dir = tempfile::Builder::new()
            .prefix("ark-turn-name-")
            .tempdir()
            .expect("temp dir");
        let state = state_with(dir.path(), RecordingBroker::new(false));
        let thread = state.store.create_thread("新会话").await.expect("thread");

        start_turn_as(
            &state,
            thread.thread_id,
            TurnSource::User("帮我评审这段   Rust 代码".to_string()),
            "帮我评审这段   Rust 代码".to_string(),
            Vec::new(),
        )
        .await
        .expect("the turn starts");

        // The runner is immediate, so by the time `start_turn_as` returns the
        // worker has run: a title that arrives earlier must already be here.
        let renamed = state
            .store
            .get_thread(thread.thread_id)
            .await
            .expect("thread");
        assert_eq!(renamed.title, "帮我评审这段 Rust 代码");
        let events = state
            .store
            .replay_events(thread.thread_id, 0)
            .await
            .expect("events");
        assert!(matches!(
            events.first().expect("the first event"),
            ark_contract::RuntimeEvent::ThreadUpdated { title, .. }
                if title == "帮我评审这段 Rust 代码"
        ));

        // A thread the user renamed keeps its title across further turns.
        state
            .store
            .rename_thread(thread.thread_id, "我的名字")
            .await
            .expect("rename");
        start_turn_as(
            &state,
            thread.thread_id,
            TurnSource::User("再来一轮".to_string()),
            "再来一轮".to_string(),
            Vec::new(),
        )
        .await
        .expect("the second turn starts");
        assert_eq!(
            state
                .store
                .get_thread(thread.thread_id)
                .await
                .expect("thread")
                .title,
            "我的名字"
        );
    }
}
