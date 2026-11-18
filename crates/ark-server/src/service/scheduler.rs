//! Scheduled-task execution loop (requirements/scheduled-tasks.md «调度与执行»).
//!
//! The service owns the *loop* and the *fold*, never the cadence maths: when a
//! task is due comes from `ark_schedule`, what a run is comes from
//! `crate::service::turns::start_turn`, and this module only decides when
//! to ask and what to write back.
//!
//! Three rules shape the code below:
//!
//! * **A run is a real turn.** It creates a thread and starts a turn through the
//!   same `start_turn` the HTTP route uses, so the text limit, the one-turn-per-
//!   thread conflict, the workspace lease and the attachment check all apply
//!   unchanged. A prompt injected as a fake event stream would be a second,
//!   weaker execution path.
//! * **No catch-up.** A trigger that passed while the process was not running is
//!   recorded as `skipped` and the cadence advances. Replaying a night of missed
//!   runs at startup would burn model budget the user never asked for.
//! * **Serial.** One scheduled run at a time (v1). The document's own
//!   `last_run.status == running` is the durable in-flight marker, so a restart
//!   mid-run is reconciled instead of being silently forgotten.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ark_common::text::truncate_utf8;
use ark_common::{ArkError, ArkErrorCode, Result, ThreadId};
use ark_contract::item::ItemKind;
use ark_contract::{
    RunStatus, ScheduledRunRecord, ScheduledTask, ThreadRuntimeSettings, DEFAULT_SCHEDULE_AUTONOMY,
};
use ark_ports::ScheduleStore;
use ark_schedule::{apply_next_run_at, is_due, parse_stored_next_run, push_record, record_run};
use chrono::{DateTime, Local, Utc};

use crate::state::AppState;

/// How often the loop looks for due tasks.
pub const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// How late a trigger may be and still run.
///
/// A healthy loop is at most one tick late, so anything beyond a few ticks can
/// only mean the process was not running (or the machine slept). Those are the
/// documented "no catch-up" cases, not something to execute late.
const MISFIRE_GRACE_SECS: i64 = 300;

/// How often a run re-reads the turn registry while waiting for its turn.
///
/// The registry has no finish notification to subscribe to (`TurnRegistry::finish`
/// only records state), and a scheduled run is a single long-lived wait against a
/// value that is already in memory, so polling the map is both the cheaper and
/// the more honest option: it cannot miss a terminal state the way a lost wakeup
/// can, and it needs no second broadcast channel that only this caller reads.
const TURN_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Upper bound on the assistant text copied into `last_run.message`.
const RUN_SUMMARY_MAX_BYTES: usize = 400;

/// Why a `skipped` record was written.
const SKIPPED_PREFIX: &str = "missed while the runtime was not running";
const SKIPPED_BUSY: &str = "previous run was still in flight";

/// Message recorded when a run outlives the process that started it.
const RESTART_ABORTED: &str = "runtime restarted before the scheduled run finished";

/// Whether a task should fire at this instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DueState {
    /// No stored fire time, disabled, or still in the future.
    NotDue,
    /// Due within the grace window: run it.
    Due,
    /// Due so long ago that executing it would be a catch-up run.
    Misfire,
}

/// Classify one task against `now`.
///
/// `is_due` alone cannot tell "two seconds late" from "two days late", and the
/// two have opposite handling, which is why the stored instant is inspected once
/// more here instead of adding a second notion of "due" to `ark-schedule`.
fn classify(task: &ScheduledTask, now: DateTime<Local>) -> DueState {
    if !task.enabled || !is_due(task, now) {
        return DueState::NotDue;
    }
    match parse_stored_next_run(&task.next_run_at) {
        Some(due_at) if (now - due_at).num_seconds() > MISFIRE_GRACE_SECS => DueState::Misfire,
        Some(_) => DueState::Due,
        // `is_due` already rejected an unparsable value; treat it the same way
        // rather than firing on garbage.
        None => DueState::NotDue,
    }
}

/// Removes the task id from the in-flight set when the run ends, by any path.
///
/// A guard instead of a trailing `release()` call: a panic inside the run (or an
/// early `?`) must not strand the id and make the task answer 409 forever.
struct InFlightGuard {
    id: String,
    set: Arc<Mutex<HashSet<String>>>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

/// The scheduler: task table plus the one-at-a-time run gate.
pub struct SchedulerService {
    store: Arc<dyn ScheduleStore>,
    /// Task ids with a run in flight. Process-local on purpose: the durable
    /// marker is `last_run.status`, this only closes the race between two
    /// callers that both read "not running" at the same instant.
    in_flight: Arc<Mutex<HashSet<String>>>,
    /// Keeps `spawn` idempotent when the composition root is built twice in one
    /// process (tests, embedded use).
    started: AtomicBool,
}

impl SchedulerService {
    pub fn new(store: Arc<dyn ScheduleStore>) -> Self {
        Self {
            store,
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            started: AtomicBool::new(false),
        }
    }

    /// The task table, for the CRUD routes.
    pub fn store(&self) -> &dyn ScheduleStore {
        self.store.as_ref()
    }

    /// Whether this task currently has a run in flight.
    pub fn is_running(&self, id: &str) -> bool {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(id)
    }

    /// Claim the run slot, or explain why the task is already busy.
    fn claim(&self, id: &str) -> Option<InFlightGuard> {
        let mut set = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !set.insert(id.to_string()) {
            return None;
        }
        Some(InFlightGuard {
            id: id.to_string(),
            set: Arc::clone(&self.in_flight),
        })
    }

    /// Whether the loop has already been started. Read-only guard for tests:
    /// assembly must hand out the handle without starting the loop.
    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    /// Start the loop. Returns immediately; the task is detached.
    ///
    /// Reconciliation runs once, before the first tick, so the state a restart
    /// left behind (a run marked `running`, triggers that passed while the
    /// process was down) is settled before anything new fires.
    pub fn spawn(self: &Arc<Self>, state: AppState) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service.reconcile().await;
            let mut shutdown = state.shutdown_receiver();
            loop {
                // Two shutdown paths reach a running loop: `/v1/shutdown`
                // broadcasts, while `Ctrl-C` and the drain only set the flag.
                // Checking it here means the flag path also converges, at worst
                // one tick late, instead of sleeping until the process drops.
                if state.is_shutting_down() {
                    break;
                }
                service.tick(&state).await;
                tokio::select! {
                    _ = tokio::time::sleep(TICK_INTERVAL) => {}
                    _ = shutdown.recv() => break,
                }
            }
            tracing::info!("scheduler loop stopped");
        });
    }

    /// Settle whatever the previous process left behind.
    ///
    /// Two repairs, both required for the document to stay honest:
    /// a `running` record from a crashed process becomes `aborted` (the turn it
    /// pointed at was already aborted by the turn-recovery pass), and a trigger
    /// that passed while the process was down becomes `skipped` with a new
    /// future `next_run_at`.
    ///
    /// Repair deliberately ignores the `schedules.enabled` switch: the switch
    /// gates *triggering* (tick / run_now), never repair. Gating repair too
    /// would leave a crash's `running` marker stuck on screen until the user
    /// re-enabled schedules — the settings page would lie for an unbounded
    /// time about a run that no longer exists.
    pub async fn reconcile(&self) {
        let tasks = match self.store.list() {
            Ok(tasks) => tasks,
            Err(err) => {
                tracing::warn!(error = %err.message, "cannot read scheduled tasks during reconcile");
                return;
            }
        };
        let now = Local::now();
        let mut repaired = 0;
        for mut task in tasks {
            let mut changed = settle_stale_run(&mut task);
            if classify(&task, now) == DueState::Misfire {
                record_run(
                    &mut task,
                    skipped_record(SKIPPED_PREFIX, now.with_timezone(&Utc)),
                );
                apply_next_run_at(&mut task, now);
                changed = true;
            }
            if !changed {
                continue;
            }
            if let Err(err) = self.store.replace(task) {
                tracing::warn!(error = %err.message, "cannot persist a reconciled scheduled task");
                continue;
            }
            repaired += 1;
        }
        if repaired != 0 {
            tracing::info!(tasks = repaired, "reconciled scheduled tasks after startup");
        }
    }

    /// One pass over the task table: run everything due, in fire order.
    ///
    /// Returns how many runs were started. The pass is serial by design (v1);
    /// ordering by `next_run_at` keeps the oldest trigger first so a slow run
    /// cannot starve an older one.
    pub async fn tick(&self, state: &AppState) -> usize {
        // The master switch is read live, so disabling schedules takes effect on
        // the next tick without a restart and without touching the task table.
        if !state.config.snapshot().schedules.enabled {
            return 0;
        }
        let tasks = match self.store.list() {
            Ok(tasks) => tasks,
            Err(err) => {
                tracing::warn!(error = %err.message, "cannot read scheduled tasks");
                return 0;
            }
        };
        let now = Local::now();
        let mut due: Vec<ScheduledTask> = Vec::new();
        for mut task in tasks {
            match classify(&task, now) {
                DueState::NotDue => {}
                DueState::Misfire => {
                    record_run(
                        &mut task,
                        skipped_record(SKIPPED_PREFIX, now.with_timezone(&Utc)),
                    );
                    apply_next_run_at(&mut task, now);
                    self.persist(task, "skipped a missed scheduled run");
                }
                DueState::Due => due.push(task),
            }
        }
        due.sort_by(|a, b| a.next_run_at.cmp(&b.next_run_at));

        let mut started = 0;
        for task in due {
            if state.is_shutting_down() {
                break;
            }
            // A task that is due again while its previous run is still going is
            // skipped and advanced, never queued: two model loops over one task
            // would double the budget and interleave their history.
            let Some(guard) = self.claim(&task.id) else {
                let mut task = task;
                record_run(
                    &mut task,
                    skipped_record(SKIPPED_BUSY, now.with_timezone(&Utc)),
                );
                apply_next_run_at(&mut task, now);
                self.persist(task, "skipped a scheduled run that was already in flight");
                continue;
            };
            match self.begin(state, task).await {
                Ok((task, thread_id, started_at)) => {
                    started += 1;
                    let _ = self.drive(state, task, thread_id, started_at).await;
                }
                Err(err) => {
                    tracing::warn!(error = %err.message, "cannot start a scheduled run");
                }
            }
            drop(guard);
        }
        started
    }

    /// Run one task now, ignoring its cadence (`POST /v1/schedules/{id}/run`).
    ///
    /// The thread and the `running` marker are written before returning so the
    /// response already reflects the state the client is about to watch; the
    /// turn itself keeps going in the background — exactly like
    /// `POST /v1/threads/{id}/turns`, which answers `202` and streams progress.
    pub async fn run_now(self: &Arc<Self>, state: &AppState, id: &str) -> Result<ScheduledTask> {
        if !state.config.snapshot().schedules.enabled {
            return Err(ArkError::new(
                ArkErrorCode::Conflict,
                "scheduled tasks are disabled by configuration",
            ));
        }
        let task = self.store.get(id)?;
        // Aligned with tick (`classify` treats a disabled task as `NotDue`):
        // a task the user turned off must not be startable by hand either.
        if !task.enabled {
            return Err(ArkError::new(
                ArkErrorCode::Conflict,
                format!("scheduled task `{id}` is disabled; enable it before running it manually"),
            ));
        }
        // Claim before creating anything: `is_running` alone would let a tick
        // claim the same task between this check and the thread insert, and the
        // losing side would have already created a thread nobody owns.
        let Some(guard) = self.claim(id) else {
            return Err(ArkError::new(
                ArkErrorCode::Conflict,
                format!("scheduled task `{id}` is already running"),
            ));
        };
        let (task, thread_id, started_at) = match self.begin(state, task).await {
            Ok(started) => started,
            // `guard` drops on this path, releasing the slot a failed start
            // never got to use.
            Err(err) => return Err(err),
        };
        let service = Arc::clone(self);
        let state = state.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let _ = service.drive(&state, task, thread_id, started_at).await;
        });
        self.store.get(id)
    }

    /// Create the run's thread, apply its runtime overrides and write the
    /// `running` marker. Returns the stored task and the new thread.
    ///
    /// The slot is claimed here rather than by the caller so a `run_now` and a
    /// tick can never both pass the "is it running?" read: the in-flight set is
    /// authoritative from the moment the thread exists.
    async fn begin(
        &self,
        state: &AppState,
        mut task: ScheduledTask,
    ) -> Result<(ScheduledTask, ThreadId, DateTime<Utc>)> {
        let thread = crate::service::threads::create_thread(state, &task.title, None).await?;
        // A scheduled run is unattended by default: `unattended` is what makes
        // the sandbox policy stop asking a human who is not there. The task's
        // own override wins when it names one.
        let autonomy = task
            .autonomy
            .clone()
            .unwrap_or_else(|| DEFAULT_SCHEDULE_AUTONOMY.to_string());
        state
            .store
            .set_thread_runtime(
                thread.thread_id,
                ThreadRuntimeSettings {
                    workspace: task.workspace.clone(),
                    provider_id: task.provider_id.clone(),
                    model: task.model.clone(),
                    autonomy: Some(autonomy),
                    reasoning_effort: None,
                },
            )
            .await?;
        // The in-flight marker lives in `last_run` only. The bounded history is
        // a log of outcomes, so writing `running` into it would leave one stale
        // line behind for every completed run - two entries where the document
        // promises one run.
        // The terminal record reuses this instant, so `last_run` keeps the real
        // start of the run instead of a zero-length interval.
        let started_at = Utc::now();
        task.last_run = Some(ScheduledRunRecord {
            status: RunStatus::Running,
            message: String::new(),
            thread_id: Some(thread.thread_id),
            started_at,
            finished_at: None,
        });
        self.persist(task.clone(), "cannot persist the running marker");
        Ok((task, thread.thread_id, started_at))
    }

    /// Start the turn and write its terminal state back onto the task.
    ///
    /// Never returns `Err`: a scheduled run that cannot start is itself a run
    /// outcome, and leaving the task marked `running` would be the one state the
    /// UI cannot recover from.
    async fn drive(
        &self,
        state: &AppState,
        mut task: ScheduledTask,
        thread_id: ThreadId,
        started_at: DateTime<Utc>,
    ) -> ScheduledTask {
        let outcome = match crate::service::start_turn_as(
            state,
            thread_id,
            crate::TurnSource::Scheduled(task.prompt.clone()),
            task.prompt.clone(),
            Vec::new(),
        )
        .await
        {
            Ok(summary) => {
                let turn_id = summary.turn_id;
                await_turn(state, turn_id).await
            }
            Err(error) => Err(error.message),
        };

        let record = match outcome {
            Ok(summary) => match summary.state.as_str() {
                "completed" => terminal_record(
                    RunStatus::Completed,
                    run_summary(state, thread_id).await,
                    thread_id,
                    started_at,
                ),
                "aborted" => terminal_record(
                    RunStatus::Aborted,
                    summary
                        .error
                        .clone()
                        .unwrap_or_else(|| "scheduled run was aborted".to_string()),
                    thread_id,
                    started_at,
                ),
                other => terminal_record(
                    RunStatus::Failed,
                    summary
                        .error
                        .clone()
                        .unwrap_or_else(|| format!("scheduled run ended as `{other}`")),
                    thread_id,
                    started_at,
                ),
            },
            Err(message) => terminal_record(RunStatus::Failed, message, thread_id, started_at),
        };

        record_run(&mut task, record);
        // The cadence is recomputed from *now*, not from the stored time: an
        // interval task counts from completion, so a run that
        // took 10 minutes does not fire again 5 minutes later.
        apply_next_run_at(&mut task, Local::now());
        let rounded = task.clone();
        self.persist(task, "cannot persist a finished scheduled run");
        rounded
    }

    fn persist(&self, task: ScheduledTask, context: &str) {
        if let Err(err) = self.store.replace(task) {
            tracing::warn!(error = %err.message, "{context}");
        }
    }
}

/// Wait for one specific turn to reach a terminal state.
///
/// The registry is the authority: it is written by the same supervisor that owns
/// the turn, on every path including a panic, so a terminal summary is exactly
/// the outcome the run must report. Two exits are not `Ok` on purpose — a closed
/// runtime and an evicted record both mean "we no longer know", which must not be
/// rounded up to success.
async fn await_turn(
    state: &AppState,
    turn_id: ark_common::TurnId,
) -> std::result::Result<ark_contract::TurnSummary, String> {
    let mut seen = false;
    loop {
        match state.turns.get(turn_id) {
            Some(summary) if summary.ended_at.is_some() => return Ok(summary),
            Some(_) => seen = true,
            // Terminal slots are evicted on a bounded window. Once the record we
            // were watching is gone, waiting longer cannot recover it; reporting
            // it beats spinning for the lifetime of the process.
            None if seen => {
                return Err("scheduled run ended without a retained turn record".to_string())
            }
            None => {}
        }
        if state.is_shutting_down() {
            return Err("runtime shut down before the scheduled run finished".to_string());
        }
        tokio::time::sleep(TURN_POLL_INTERVAL).await;
    }
}

/// Close the `running` marker a previous process left behind.
///
/// Returns whether anything changed. The marker lives in `last_run` alone, so
/// settling it must still produce one terminal history line: a run that was cut
/// short is an outcome like any other, and `last_run` disagreeing with the log
/// is the one state the settings page cannot render.
fn settle_stale_run(task: &mut ScheduledTask) -> bool {
    let Some(running) = task
        .last_run
        .as_ref()
        .filter(|record| record.status == RunStatus::Running)
        .cloned()
    else {
        return false;
    };
    let settled = ScheduledRunRecord {
        status: RunStatus::Aborted,
        message: RESTART_ABORTED.to_string(),
        thread_id: running.thread_id,
        started_at: running.started_at,
        finished_at: Some(Utc::now()),
    };
    // A `running` line in the log can only come from a document written before
    // the marker moved to `last_run`; rewrite it in place instead of appending a
    // second line that describes the same run.
    match task
        .history
        .iter_mut()
        .find(|record| record.status == RunStatus::Running)
    {
        Some(entry) => *entry = settled.clone(),
        None => push_record(&mut task.history, settled.clone()),
    }
    task.last_run = Some(settled);
    true
}

fn skipped_record(reason: &str, at: DateTime<Utc>) -> ScheduledRunRecord {
    ScheduledRunRecord {
        status: RunStatus::Skipped,
        message: reason.to_string(),
        thread_id: None,
        started_at: at,
        finished_at: Some(at),
    }
}

fn terminal_record(
    status: RunStatus,
    message: String,
    thread_id: ThreadId,
    started_at: DateTime<Utc>,
) -> ScheduledRunRecord {
    ScheduledRunRecord {
        status,
        message: truncate_utf8(message.trim(), RUN_SUMMARY_MAX_BYTES).to_string(),
        thread_id: Some(thread_id),
        started_at,
        finished_at: Some(Utc::now()),
    }
}

/// Last non-empty assistant message of the run, bounded.
///
/// Read from the thread's own projection instead of the model response: the
/// stored items are what the user will see when they open the transcript, so the
/// summary and the opened thread can never disagree.
async fn run_summary(state: &AppState, thread_id: ThreadId) -> String {
    let Ok(items) = state.store.read_items(thread_id).await else {
        return String::new();
    };
    items
        .iter()
        .rev()
        .filter(|item| item.kind == ItemKind::AssistantMessage)
        .find_map(|item| {
            item.text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
        .map(|text| truncate_utf8(text, RUN_SUMMARY_MAX_BYTES).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
