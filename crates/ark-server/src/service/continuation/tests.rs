use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use ark_common::{ChildId, Result, Usage};
use ark_config::{ConfigStore, RuntimeConfig};
use ark_contract::RuntimeEvent;
use ark_obs::UsageRecorder;
use ark_ports::{SessionStore, ToolRegistryPort};
use ark_store::{FileSessionStore, StoreConfig};
use chrono::Utc;

use super::*;
use crate::state::{TurnEnd, TurnRunner};

static COMPLETION_READ_FAILURES: OnceLock<Mutex<HashSet<ThreadId>>> = OnceLock::new();

fn fail_next_completion_read(thread_id: ThreadId) {
    COMPLETION_READ_FAILURES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(thread_id);
}

pub(super) fn take_completion_read_failure(thread_id: ThreadId) -> bool {
    COMPLETION_READ_FAILURES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&thread_id)
}

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
impl TurnRunner for CountingRunner {
    async fn run_turn(&self, job: TurnJob) -> Result<TurnEnd> {
        assert!(matches!(job.source, TurnSource::AutoContinuation));
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(TurnEnd::Completed)
    }
}

struct Harness {
    state: AppState,
    store: Arc<FileSessionStore>,
    runner: Arc<CountingRunner>,
    root: std::path::PathBuf,
}

impl Harness {
    fn new(enabled: bool) -> Self {
        let root = std::env::temp_dir().join(format!("ark-auto-continue-{}", TurnId::new()));
        let store = Arc::new(FileSessionStore::open(StoreConfig::new(&root)).unwrap());
        let config = RuntimeConfig {
            subagents: ark_contract::SubagentSettings {
                auto_continue: enabled,
                ..ark_contract::SubagentSettings::default()
            },
            ..RuntimeConfig::default()
        };
        let runner = Arc::new(CountingRunner(AtomicUsize::new(0)));
        let state = AppState::new(
            store.clone() as Arc<dyn SessionStore>,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(config)),
            Arc::new(UsageRecorder::new()),
            runner.clone(),
            root.clone(),
            "test",
        );
        Self {
            state,
            store,
            runner,
            root,
        }
    }

    async fn append_completion(&self, thread: ThreadId, index: usize) {
        self.store
            .append_event(
                thread,
                RuntimeEvent::ChildCompleted {
                    seq: 0,
                    thread_id: thread,
                    timestamp: Utc::now(),
                    notification_id: format!("child:{index}:resume:0"),
                    child_id: ChildId::new(),
                    label: "worker".into(),
                    status: "completed".into(),
                    summary: Some("done".into()),
                    error: None,
                    usage: Some(Usage::default()),
                    tool_invocations: Some(0),
                },
            )
            .await
            .unwrap();
    }

    async fn thread_with_completion(&self) -> ThreadId {
        let thread = self.store.create_thread("parent").await.unwrap().thread_id;
        self.append_completion(thread, 0).await;
        thread
    }

    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.state.begin_shutdown();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn default_policy_never_starts_a_model_turn() {
    let harness = Harness::new(false);
    let thread = harness.thread_with_completion().await;
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn completion_burst_starts_one_continuation() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    for _ in 0..6 {
        schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(10)).await;
    }
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn active_archived_and_deleted_threads_do_not_continue() {
    let active = Harness::new(true);
    let active_thread = active.thread_with_completion().await;
    active
        .state
        .turns
        .try_register(active_thread, TurnId::new(), CancelToken::new());
    schedule_with_delay(
        active.state.clone(),
        active_thread,
        Duration::from_millis(5),
    )
    .await;

    let archived = Harness::new(true);
    let archived_thread = archived.thread_with_completion().await;
    archived
        .store
        .set_archived(archived_thread, true)
        .await
        .unwrap();
    schedule_with_delay(
        archived.state.clone(),
        archived_thread,
        Duration::from_millis(5),
    )
    .await;

    let deleted = Harness::new(true);
    let deleted_thread = deleted.thread_with_completion().await;
    deleted.store.delete_thread(deleted_thread).await.unwrap();
    schedule_with_delay(
        deleted.state.clone(),
        deleted_thread,
        Duration::from_millis(5),
    )
    .await;

    active.settle().await;
    archived.settle().await;
    deleted.settle().await;
    assert_eq!(active.runner.0.load(Ordering::SeqCst), 0);
    assert_eq!(archived.runner.0.load(Ordering::SeqCst), 0);
    assert_eq!(deleted.runner.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancelling_pending_debounce_prevents_continuation() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(40)).await;
    assert!(harness.state.continuations.cancel(thread));
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 0);
    assert!(!harness.state.continuations.contains(thread));
}

#[tokio::test]
async fn disabling_policy_during_debounce_prevents_continuation() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(40)).await;
    let mut settings = harness.state.config.snapshot().subagents;
    settings.auto_continue = false;
    harness.state.config.apply(ark_contract::ConfigPatch {
        subagents: Some(settings),
        ..Default::default()
    });
    sync_auto_continue_policy(harness.state.clone()).await;
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn disabling_subagents_cancels_pending_and_rejects_new_wakeups() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(40)).await;
    let mut settings = harness.state.config.snapshot().subagents;
    settings.enabled = false;
    harness.state.config.apply(ark_contract::ConfigPatch {
        subagents: Some(settings),
        ..Default::default()
    });
    sync_auto_continue_policy(harness.state.clone()).await;
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 0);
    assert!(!harness.state.continuations.contains(thread));
}

#[tokio::test]
async fn shutdown_cancels_pending_and_rejects_late_wakeups() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(40)).await;
    harness.state.begin_shutdown();
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn suppressed_thread_rejects_late_completion_until_explicitly_allowed() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    harness.state.continuations.suppress(thread);
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 0);

    harness.state.continuations.allow(thread);
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_rechecks_durable_interrupt_before_starting() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    harness
        .store
        .append_event(
            thread,
            RuntimeEvent::TurnAborted {
                seq: 0,
                thread_id: thread,
                turn_id: TurnId::new(),
                timestamp: Utc::now(),
                reason: "turn interrupted".into(),
            },
        )
        .await
        .unwrap();
    fail_next_completion_read(thread);

    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while harness.state.continuations.contains(thread) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("durable interrupt should clear the retrying slot");

    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_scan_cannot_suppress_a_new_user_epoch() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    harness
        .store
        .append_event(
            thread,
            RuntimeEvent::TurnAborted {
                seq: 0,
                thread_id: thread,
                turn_id: TurnId::new(),
                timestamp: Utc::now(),
                reason: "turn interrupted".into(),
            },
        )
        .await
        .unwrap();

    let workspace_guard = harness.state.workspace_gate.lock().await;
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let scan_state = harness.state.clone();
    let scan = tokio::spawn(async move {
        sync_auto_continue_policy_inner(scan_state, move |scanned| {
            if scanned == thread {
                reached_tx.send(()).unwrap();
            }
        })
        .await;
    });
    tokio::task::spawn_blocking(move || reached_rx.recv().unwrap())
        .await
        .unwrap();

    let user_turn = TurnId::new();
    harness
        .state
        .turns
        .try_register(thread, user_turn, CancelToken::new())
        .expect("user lifecycle registers while owning workspace gate");
    harness.state.continuations.allow(thread);
    harness.state.turns.finish(user_turn, &TurnEnd::Completed);
    harness
        .store
        .append_event(
            thread,
            RuntimeEvent::TurnCompleted {
                seq: 0,
                thread_id: thread,
                turn_id: user_turn,
                timestamp: Utc::now(),
            },
        )
        .await
        .unwrap();
    drop(workspace_guard);
    scan.await.unwrap();

    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;

    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn startup_scan_preserves_an_explicit_interrupt() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    let turn = TurnId::new();
    harness
        .store
        .append_event(
            thread,
            RuntimeEvent::TurnAborted {
                seq: 0,
                thread_id: thread,
                turn_id: turn,
                timestamp: Utc::now(),
                reason: "turn interrupted".into(),
            },
        )
        .await
        .unwrap();

    sync_auto_continue_policy(harness.state.clone()).await;
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn startup_scan_does_not_treat_shutdown_as_user_interrupt() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    harness
        .store
        .append_event(
            thread,
            RuntimeEvent::TurnAborted {
                seq: 0,
                thread_id: thread,
                turn_id: TurnId::new(),
                timestamp: Utc::now(),
                reason: crate::state::SHUTDOWN_CANCEL_REASON.into(),
            },
        )
        .await
        .unwrap();

    assert!(!interrupted_user_lifecycle(&harness.state, thread).await);
    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(harness.runner.0.load(Ordering::SeqCst), 1);
}

// ---- auto-continuation bounds (notification attempt cap + model-failure breaker) ----

impl Harness {
    async fn append_completion_with_id(&self, thread: ThreadId, notification_id: &str) {
        self.store
            .append_event(
                thread,
                RuntimeEvent::ChildCompleted {
                    seq: 0,
                    thread_id: thread,
                    timestamp: Utc::now(),
                    notification_id: notification_id.to_string(),
                    child_id: ChildId::new(),
                    label: "worker".into(),
                    status: "failed".into(),
                    summary: None,
                    error: Some("gateway 503".into()),
                    usage: None,
                    tool_invocations: None,
                },
            )
            .await
            .unwrap();
    }

    async fn append_auto_turn_started(&self, thread: ThreadId) {
        self.store
            .append_event(
                thread,
                RuntimeEvent::TurnStarted {
                    seq: 0,
                    thread_id: thread,
                    turn_id: TurnId::new(),
                    timestamp: Utc::now(),
                    auto_continuation: true,
                },
            )
            .await
            .unwrap();
    }

    async fn append_user_turn_started(&self, thread: ThreadId) {
        self.store
            .append_event(
                thread,
                RuntimeEvent::TurnStarted {
                    seq: 0,
                    thread_id: thread,
                    turn_id: TurnId::new(),
                    timestamp: Utc::now(),
                    auto_continuation: false,
                },
            )
            .await
            .unwrap();
    }

    async fn append_turn_failed(&self, thread: ThreadId, model_request_failed: bool) {
        self.store
            .append_event(
                thread,
                RuntimeEvent::TurnFailed {
                    seq: 0,
                    thread_id: thread,
                    turn_id: TurnId::new(),
                    timestamp: Utc::now(),
                    message: "model request failed".into(),
                    model_request_failed,
                },
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn poison_notification_stops_scheduling_after_the_attempt_cap() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    // Two automatic continuations already delivered this notification.
    harness.append_auto_turn_started(thread).await;
    harness.append_auto_turn_started(thread).await;

    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(
        harness.runner.0.load(Ordering::SeqCst),
        0,
        "notification at the attempt cap must not schedule another model turn"
    );
}

#[tokio::test]
async fn consecutive_model_request_failures_pause_auto_continue() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    harness.append_auto_turn_started(thread).await;
    harness.append_turn_failed(thread, true).await;
    harness.append_auto_turn_started(thread).await;
    harness.append_turn_failed(thread, true).await;

    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(
        harness.runner.0.load(Ordering::SeqCst),
        0,
        "the circuit breaker must stop scheduling after repeated model failures"
    );
}

#[tokio::test]
async fn non_model_failure_does_not_trip_the_breaker() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    // One automatic delivery (below the per-notification cap) that failed on
    // something other than the model request.
    harness.append_auto_turn_started(thread).await;
    harness.append_turn_failed(thread, false).await;
    harness.append_turn_failed(thread, false).await;

    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(
        harness.runner.0.load(Ordering::SeqCst),
        1,
        "tool/policy failures are not model outages and keep the bounded drain"
    );
}

#[tokio::test]
async fn fresh_notification_after_capped_one_still_schedules() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    // The old notification reached the cap, then a new child finished.
    harness.append_auto_turn_started(thread).await;
    harness.append_auto_turn_started(thread).await;
    harness
        .append_completion_with_id(thread, "child:new:resume:0")
        .await;

    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(
        harness.runner.0.load(Ordering::SeqCst),
        1,
        "the cap is per notification, not per thread"
    );
}

#[tokio::test]
async fn acknowledged_success_still_drains_remaining_notifications() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    // A completed automatic turn acknowledged the first notification.
    harness.append_auto_turn_started(thread).await;
    harness
        .store
        .append_event(
            thread,
            RuntimeEvent::ChildCompletionAcknowledged {
                seq: 0,
                thread_id: thread,
                turn_id: TurnId::new(),
                timestamp: Utc::now(),
                notification_id: "child:0:resume:0".into(),
            },
        )
        .await
        .unwrap();
    // A second notification is still unacknowledged.
    harness
        .append_completion_with_id(thread, "child:1:resume:0")
        .await;

    schedule_with_delay(harness.state.clone(), thread, Duration::from_millis(5)).await;
    harness.settle().await;
    assert_eq!(
        harness.runner.0.load(Ordering::SeqCst),
        1,
        "successful acknowledgement keeps the bounded drain going"
    );
}

#[tokio::test]
async fn user_turn_resets_attempts_and_consumes_capped_notification() {
    let harness = Harness::new(true);
    let thread = harness.thread_with_completion().await;
    harness.append_auto_turn_started(thread).await;
    harness.append_auto_turn_started(thread).await;
    // A user turn opens a new lifecycle epoch and consumes the notification.
    harness.append_user_turn_started(thread).await;
    harness
        .store
        .append_event(
            thread,
            RuntimeEvent::TurnCompleted {
                seq: 0,
                thread_id: thread,
                turn_id: TurnId::new(),
                timestamp: Utc::now(),
            },
        )
        .await
        .unwrap();

    // The user epoch resets the attempt accounting, so the notification is
    // deliverable again on both paths until it is actually acknowledged.
    let events = harness
        .store
        .replay_child_completion_events(thread)
        .await
        .unwrap();
    let merged = {
        let mut merged = events.clone();
        let lifecycle = harness
            .store
            .replay_interaction_events(thread)
            .await
            .unwrap();
        merged.extend(lifecycle);
        merged.sort_by_key(|event| event.seq());
        merged
    };
    assert_eq!(
        CompletionInbox::from_events_for_auto_continuation(&merged)
            .notification_ids()
            .len(),
        1,
        "a user turn resets the per-notification attempt cap"
    );
    assert_eq!(
        CompletionInbox::from_events(&events)
            .notification_ids()
            .len(),
        1,
        "an unacknowledged notification remains available to user turns"
    );
}

// ---- lifecycle completeness through the real continuation supervisor ----

/// Stands in for the gateway-outage path: the model request fails before the
/// reactor journals `turn_started`.
struct ModelFailureRunner(AtomicUsize);

#[async_trait::async_trait]
impl TurnRunner for ModelFailureRunner {
    async fn run_turn(&self, job: TurnJob) -> Result<TurnEnd> {
        assert!(matches!(job.source, TurnSource::AutoContinuation));
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ark_common::ArkError::model(
            "gateway 503 no healthy account",
        ))
    }
}

/// `(turn_started count, terminal count)` for one turn.
fn lifecycle_of(events: &[RuntimeEvent], turn: TurnId) -> (usize, usize) {
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

/// Wait until the supervisor has produced `expected` continuation turns.
async fn wait_for_turns(runner: &ModelFailureRunner, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while runner.0.load(Ordering::SeqCst) < expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the continuation supervisor starts the expected turns");
}

#[tokio::test]
async fn failed_continuations_keep_a_complete_lifecycle_until_the_breaker_holds() {
    let root = std::env::temp_dir().join(format!("ark-cont-lifecycle-{}", TurnId::new()));
    let store = Arc::new(FileSessionStore::open(StoreConfig::new(&root)).unwrap());
    let config = RuntimeConfig {
        subagents: ark_contract::SubagentSettings {
            auto_continue: true,
            ..ark_contract::SubagentSettings::default()
        },
        ..RuntimeConfig::default()
    };
    let runner = Arc::new(ModelFailureRunner(AtomicUsize::new(0)));
    let state = AppState::new(
        store.clone() as Arc<dyn SessionStore>,
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(config)),
        Arc::new(UsageRecorder::new()),
        runner.clone(),
        root.clone(),
        "test",
    );
    let thread = store.create_thread("parent").await.unwrap().thread_id;
    store
        .append_event(
            thread,
            RuntimeEvent::ChildCompleted {
                seq: 0,
                thread_id: thread,
                timestamp: Utc::now(),
                notification_id: "child:0:resume:0".into(),
                child_id: ChildId::new(),
                label: "worker".into(),
                status: "completed".into(),
                summary: Some("done".into()),
                error: None,
                usage: Some(Usage::default()),
                tool_invocations: Some(0),
            },
        )
        .await
        .unwrap();

    // Child completion wakes the supervisor twice; each continuation fails on the
    // model request. The breaker then holds, so a third wakeup starts nothing.
    for expected in 1..=2 {
        schedule_with_delay(state.clone(), thread, Duration::from_millis(5)).await;
        wait_for_turns(&runner, expected).await;
    }
    schedule_with_delay(state.clone(), thread, Duration::from_millis(5)).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        runner.0.load(Ordering::SeqCst),
        2,
        "the circuit breaker must stop scheduling after two model failures"
    );

    let events = store.replay_interaction_events(thread).await.unwrap();
    let turns: Vec<TurnId> = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::TurnStarted {
                turn_id,
                auto_continuation: true,
                ..
            } => Some(*turn_id),
            _ => None,
        })
        .collect();
    assert_eq!(turns.len(), 2, "each started continuation is recorded once");
    for turn in turns {
        assert_eq!(
            lifecycle_of(&events, turn),
            (1, 1),
            "a failed continuation turn still has one start and one terminal record"
        );
    }
    assert_eq!(
        ark_agent::consecutive_auto_model_request_failures(&events),
        2,
        "the breaker reads the same durable records the audit trail shows"
    );

    state.begin_shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
