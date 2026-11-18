use super::*;
use ark_contract::{ScheduleKind, TaskSchedule};
use chrono::TimeZone;

fn task(kind: ScheduleKind, next_run_at: DateTime<Local>) -> ScheduledTask {
    ScheduledTask {
        id: "sch_test".to_string(),
        title: "nightly".to_string(),
        enabled: true,
        prompt: "do the thing".to_string(),
        workspace: None,
        autonomy: None,
        model: None,
        provider_id: None,
        schedule: TaskSchedule {
            kind,
            every_minutes: 30,
            time_of_day: "09:00".to_string(),
            at_time: String::new(),
            day_of_week: vec![1],
            day_of_month: vec![1],
        },
        created_at: next_run_at.with_timezone(&Utc),
        updated_at: next_run_at.with_timezone(&Utc),
        last_run: None,
        next_run_at: next_run_at.to_rfc3339(),
        history: Vec::new(),
    }
}

fn at_local(hour: u32, minute: u32) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(2026, 11, 1, hour, minute, 0)
        .single()
        .expect("unambiguous local instant")
}

#[test]
fn a_freshly_due_task_runs_instead_of_being_skipped() {
    let now = at_local(9, 0);
    let task = task(ScheduleKind::Daily, now - chrono::Duration::seconds(20));
    assert_eq!(classify(&task, now), DueState::Due);
}

#[test]
fn a_trigger_missed_while_the_runtime_was_down_is_a_misfire() {
    let now = at_local(9, 0);
    let task = task(ScheduleKind::Daily, now - chrono::Duration::hours(6));
    assert_eq!(classify(&task, now), DueState::Misfire);
}

#[test]
fn a_future_or_disabled_task_is_not_due() {
    let now = at_local(9, 0);
    let future = task(ScheduleKind::Daily, now + chrono::Duration::minutes(5));
    assert_eq!(classify(&future, now), DueState::NotDue);

    let mut disabled = task(ScheduleKind::Daily, now - chrono::Duration::seconds(1));
    disabled.enabled = false;
    assert_eq!(classify(&disabled, now), DueState::NotDue);
}

#[test]
fn an_empty_or_unparsable_fire_time_never_fires() {
    let now = at_local(9, 0);
    let mut empty = task(ScheduleKind::Manual, now);
    empty.next_run_at = String::new();
    assert_eq!(classify(&empty, now), DueState::NotDue);

    let mut garbage = task(ScheduleKind::Manual, now);
    garbage.next_run_at = "not-a-time".to_string();
    assert_eq!(classify(&garbage, now), DueState::NotDue);
}

#[test]
fn a_stale_running_record_becomes_aborted_on_reconcile() {
    let now = at_local(9, 0);
    let mut task = task(ScheduleKind::Daily, now);
    record_run(
        &mut task,
        ScheduledRunRecord {
            status: RunStatus::Running,
            message: String::new(),
            thread_id: None,
            started_at: now.with_timezone(&Utc),
            finished_at: None,
        },
    );
    assert!(settle_stale_run(&mut task));
    assert_eq!(task.last_run.as_ref().unwrap().status, RunStatus::Aborted);
    assert_eq!(task.history.len(), 1);
    assert_eq!(task.history[0].status, RunStatus::Aborted);
    assert!(task.last_run.as_ref().unwrap().finished_at.is_some());
}

#[test]
fn settling_a_task_that_is_not_running_is_a_no_op() {
    let now = at_local(9, 0);
    let mut task = task(ScheduleKind::Daily, now);
    assert!(!settle_stale_run(&mut task));
    assert!(task.last_run.is_none());
}

// ---- integration: a real state, a real store, a fake executor ----------

use std::sync::Mutex as StdMutex;

use ark_config::{ConfigStore, RuntimeConfig};
use ark_contract::{ItemStatus, ScheduleSettings, ThreadItem};
use ark_obs::UsageRecorder;
use ark_ports::{SessionStore, ToolRegistryPort};
use ark_store::{FileSessionStore, ScheduleFileStore, StoreConfig};
use tokio::sync::Notify;

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

/// Fake executor: journals a real lifecycle so the service sees what the
/// production reactor would produce, and records the prompt it was handed.
struct RecordingRunner {
    store: Arc<FileSessionStore>,
    prompts: StdMutex<Vec<String>>,
    fail: bool,
    /// When present, `run_turn` waits here so a test can observe the task as
    /// "running" before it finishes.
    gate: Option<Arc<Notify>>,
}

#[async_trait::async_trait]
impl crate::TurnRunner for RecordingRunner {
    async fn run_turn(&self, job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
        if let crate::TurnSource::Scheduled(text) = &job.source {
            self.prompts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(text.clone());
        }
        let events = ark_agent::TurnEvents::new(job.thread_id, job.turn_id);
        self.store
            .append_event(job.thread_id, events.turn_started())
            .await?;
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        if self.fail {
            self.store
                .append_event(job.thread_id, events.turn_failed("model exploded"))
                .await?;
            return Ok(crate::TurnEnd::Failed {
                message: "model exploded".to_string(),
                model_request_failed: false,
            });
        }
        let item = ThreadItem::new(job.turn_id, ItemKind::AssistantMessage)
            .with_text("all done")
            .completed();
        debug_assert_eq!(item.status, ItemStatus::Completed);
        self.store
            .append_event(job.thread_id, events.item_completed(item))
            .await?;
        self.store
            .append_event(job.thread_id, events.turn_completed())
            .await?;
        Ok(crate::TurnEnd::Completed)
    }
}

struct Harness {
    state: AppState,
    service: Arc<SchedulerService>,
    store: Arc<FileSessionStore>,
    runner: Arc<RecordingRunner>,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new(enabled: bool, fail: bool, gate: Option<Arc<Notify>>) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("ark-scheduler-")
            .tempdir()
            .expect("temp dir");
        let root = dir.path().to_path_buf();
        let store = Arc::new(FileSessionStore::open(StoreConfig::new(&root)).expect("store"));
        let config = RuntimeConfig {
            schedules: ScheduleSettings { enabled },
            ..RuntimeConfig::default()
        };
        let runner = Arc::new(RecordingRunner {
            store: Arc::clone(&store),
            prompts: StdMutex::new(Vec::new()),
            fail,
            gate,
        });
        let service = Arc::new(SchedulerService::new(Arc::new(
            ScheduleFileStore::open(&root).expect("schedule store"),
        )));
        let state = AppState::new(
            Arc::clone(&store) as Arc<dyn SessionStore>,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(config)),
            Arc::new(UsageRecorder::new()),
            runner.clone(),
            root,
            "test",
        )
        .with_schedules(Arc::clone(&service));
        Self {
            state,
            service,
            store,
            runner,
            _dir: dir,
        }
    }

    fn seed(&self, task: ScheduledTask) -> ScheduledTask {
        self.service.store().insert(task).expect("insert")
    }

    fn stored(&self, id: &str) -> ScheduledTask {
        self.service.store().get(id).expect("stored task")
    }
}

fn due_task(minutes_ago: i64) -> ScheduledTask {
    task(
        ScheduleKind::Interval,
        Local::now() - chrono::Duration::minutes(minutes_ago),
    )
}

#[tokio::test]
async fn a_tick_runs_the_task_and_writes_the_terminal_record_back() {
    let harness = Harness::new(true, false, None);
    let seeded = harness.seed(due_task(0));

    let started = harness.service.tick(&harness.state).await;
    assert_eq!(started, 1, "one due task is one run");

    let prompts = harness
        .runner
        .prompts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(
        prompts,
        vec![seeded.prompt.clone()],
        "the task prompt is the turn text"
    );

    let stored = harness.stored(&seeded.id);
    let last = stored.last_run.as_ref().expect("a run was recorded");
    assert_eq!(last.status, RunStatus::Completed);
    assert_eq!(
        last.message, "all done",
        "the assistant's last text is the summary"
    );
    assert!(last.finished_at.is_some());
    let thread_id = last.thread_id.expect("the run names its thread");
    assert_eq!(
        harness
            .store
            .get_thread(thread_id)
            .await
            .expect("thread exists")
            .thread_id,
        thread_id
    );
    assert_eq!(stored.history.len(), 1, "history mirrors the run");
    assert!(
        parse_stored_next_run(&stored.next_run_at).is_some_and(|next| next > Local::now()),
        "the cadence advances past the run"
    );
}

#[tokio::test]
async fn a_failed_run_is_recorded_as_failed_not_completed() {
    let harness = Harness::new(true, true, None);
    let seeded = harness.seed(due_task(0));

    harness.service.tick(&harness.state).await;

    let last = harness.stored(&seeded.id).last_run.expect("recorded");
    assert_eq!(last.status, RunStatus::Failed);
    assert_eq!(last.message, "model exploded");
}

#[tokio::test]
async fn the_master_switch_makes_a_tick_a_no_op_without_touching_the_table() {
    let harness = Harness::new(false, false, None);
    let seeded = harness.seed(due_task(0));

    assert_eq!(harness.service.tick(&harness.state).await, 0);
    assert!(harness.runner.prompts.lock().unwrap().is_empty());
    let stored = harness.stored(&seeded.id);
    assert!(stored.last_run.is_none(), "a disabled loop records nothing");
    assert_eq!(
        stored.next_run_at, seeded.next_run_at,
        "the stored fire time is untouched"
    );
}

#[tokio::test]
async fn a_trigger_missed_while_down_is_skipped_and_never_run() {
    let harness = Harness::new(true, false, None);
    let seeded = harness.seed(due_task(6 * 60));

    assert_eq!(
        harness.service.tick(&harness.state).await,
        0,
        "no catch-up run"
    );
    assert!(harness.runner.prompts.lock().unwrap().is_empty());

    let stored = harness.stored(&seeded.id);
    let last = stored.last_run.expect("the miss is recorded");
    assert_eq!(last.status, RunStatus::Skipped);
    assert!(last.thread_id.is_none(), "a skipped run creates no thread");
    assert!(
        parse_stored_next_run(&stored.next_run_at).is_some_and(|next| next > Local::now()),
        "the cadence still advances so the next slot is future"
    );
}

#[tokio::test]
async fn run_now_rejects_a_task_whose_run_is_still_in_flight() {
    let gate = Arc::new(Notify::new());
    let harness = Harness::new(true, false, Some(Arc::clone(&gate)));
    let seeded = harness.seed(due_task(60));

    let started = harness
        .service
        .run_now(&harness.state, &seeded.id)
        .await
        .expect("the first run is accepted");
    assert_eq!(
        started.last_run.as_ref().map(|record| record.status),
        Some(RunStatus::Running),
        "the response already shows the running marker"
    );

    let conflict = harness
        .service
        .run_now(&harness.state, &seeded.id)
        .await
        .expect_err("a second run conflicts");
    assert_eq!(conflict.code, ArkErrorCode::Conflict);

    // `notify_one` (not `notify_waiters`): the background run may not have
    // reached the gate yet, and `notify_waiters` would be a lost wakeup.
    gate.notify_one();
    // The background run writes its terminal record; wait for it rather than
    // assuming a scheduling delay.
    for _ in 0..200 {
        let stored = harness.stored(&seeded.id);
        if matches!(
            stored.last_run.as_ref().map(|record| record.status),
            Some(RunStatus::Completed)
        ) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        harness
            .stored(&seeded.id)
            .last_run
            .map(|record| record.status),
        Some(RunStatus::Completed)
    );
    assert!(
        !harness.service.is_running(&seeded.id),
        "the slot is released"
    );
}

#[tokio::test]
async fn run_now_rejects_a_disabled_task_until_it_is_enabled_again() {
    let harness = Harness::new(true, false, None);
    let mut seeded = harness.seed(due_task(60));
    seeded.enabled = false;
    harness
        .service
        .store()
        .replace(seeded.clone())
        .expect("disable");

    let error = harness
        .service
        .run_now(&harness.state, &seeded.id)
        .await
        .expect_err("a disabled task is not manually runnable");
    assert_eq!(error.code, ArkErrorCode::Conflict);
    assert!(
        error.message.contains("disabled"),
        "the error explains the task must be enabled first"
    );
    assert!(harness.stored(&seeded.id).last_run.is_none());

    // Once the user re-enables the task, run_now goes through.
    seeded.enabled = true;
    harness
        .service
        .store()
        .replace(seeded.clone())
        .expect("enable");
    let started = harness
        .service
        .run_now(&harness.state, &seeded.id)
        .await
        .expect("the enabled task runs");
    assert_eq!(
        started.last_run.as_ref().map(|record| record.status),
        Some(RunStatus::Running)
    );
}

#[tokio::test]
async fn run_now_is_rejected_while_the_master_switch_is_off() {
    let harness = Harness::new(false, false, None);
    let seeded = harness.seed(due_task(60));

    let error = harness
        .service
        .run_now(&harness.state, &seeded.id)
        .await
        .expect_err("the switch wins");
    assert_eq!(error.code, ArkErrorCode::Conflict);
    assert!(harness.stored(&seeded.id).last_run.is_none());
}

#[tokio::test]
async fn a_shutdown_mid_tick_starts_nothing_new() {
    let harness = Harness::new(true, false, None);
    let seeded = harness.seed(due_task(0));
    harness.state.begin_shutdown();

    assert_eq!(harness.service.tick(&harness.state).await, 0);
    assert!(harness.stored(&seeded.id).last_run.is_none());
}

#[tokio::test]
async fn reconcile_settles_a_run_left_running_by_a_previous_process() {
    let harness = Harness::new(true, false, None);
    // Due, but not late: the crash repair is the only thing reconcile has to
    // do here, so the assertions below describe it alone.
    let mut seeded = due_task(0);
    record_run(
        &mut seeded,
        ScheduledRunRecord {
            status: RunStatus::Running,
            message: String::new(),
            thread_id: None,
            started_at: Utc::now(),
            finished_at: None,
        },
    );
    let seeded = harness.seed(seeded);

    harness.service.reconcile().await;

    let stored = harness.stored(&seeded.id);
    assert_eq!(
        stored.last_run.as_ref().map(|record| record.status),
        Some(RunStatus::Aborted),
        "a crash cannot leave a task marked running"
    );
    assert!(stored
        .last_run
        .as_ref()
        .is_some_and(|record| record.finished_at.is_some()));
    assert_eq!(
        stored.history.len(),
        1,
        "the settled run is one outcome, not a running line plus a terminal one"
    );
    assert_eq!(stored.history[0].status, RunStatus::Aborted);
}

#[tokio::test]
async fn reconcile_repairs_even_while_schedules_are_disabled() {
    // The switch gates triggering, not repair: with schedules disabled, a
    // crash's `running` marker must still settle instead of sticking on
    // screen until the user re-enables the feature.
    let harness = Harness::new(false, false, None);
    let mut seeded = due_task(0);
    record_run(
        &mut seeded,
        ScheduledRunRecord {
            status: RunStatus::Running,
            message: String::new(),
            thread_id: None,
            started_at: Utc::now(),
            finished_at: None,
        },
    );
    let seeded = harness.seed(seeded);

    harness.service.reconcile().await;

    let stored = harness.stored(&seeded.id);
    assert_eq!(
        stored.last_run.as_ref().map(|record| record.status),
        Some(RunStatus::Aborted)
    );
    // Repair never triggers: no run may start while the switch is off.
    assert!(harness
        .runner
        .prompts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty());
}

#[tokio::test]
async fn reconcile_records_the_missed_trigger_even_while_schedules_are_disabled() {
    // The other half of "repair ignores the switch": a trigger that went
    // stale while the process was down is honestly recorded as `skipped`
    // (and rescheduled) even though no run may ever start.
    let harness = Harness::new(false, false, None);
    let before = due_task(10 * 60);
    let seeded = harness.seed(before.clone());

    harness.service.reconcile().await;

    let stored = harness.stored(&seeded.id);
    assert_eq!(
        stored.last_run.as_ref().map(|record| record.status),
        Some(RunStatus::Skipped)
    );
    // The stale trigger is rescheduled into the future, not left behind.
    assert_ne!(stored.next_run_at, before.next_run_at);
    assert!(harness
        .runner
        .prompts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty());
}

#[tokio::test]
async fn reconcile_settles_the_crash_then_records_the_missed_trigger() {
    let harness = Harness::new(true, false, None);
    let mut seeded = due_task(6 * 60);
    record_run(
        &mut seeded,
        ScheduledRunRecord {
            status: RunStatus::Running,
            message: String::new(),
            thread_id: None,
            started_at: Utc::now(),
            finished_at: None,
        },
    );
    let seeded = harness.seed(seeded);

    harness.service.reconcile().await;

    // Two repairs, two outcomes, oldest first: the run that never finished,
    // then the trigger that passed while the process was down.
    let stored = harness.stored(&seeded.id);
    assert_eq!(
        stored
            .history
            .iter()
            .map(|record| record.status)
            .collect::<Vec<_>>(),
        vec![RunStatus::Aborted, RunStatus::Skipped]
    );
    assert_eq!(
        stored.last_run.as_ref().map(|record| record.status),
        Some(RunStatus::Skipped),
        "the newest entry is mirrored into last_run"
    );
}
