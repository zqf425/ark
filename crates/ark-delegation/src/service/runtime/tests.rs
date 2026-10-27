//! Delegation runtime tests.
//!
//! These run against the **real** `FileSessionStore` in a temp directory, so the
//! assertions cover the actual `children/runs.jsonl` path and the inlined child
//! directory layout rather than a fake that could drift from it.
//!
//! The executor is a fake on purpose: the reactor is `arkd`'s job to wire, and
//! these tests are about budget, lifecycle and records.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ark_common::{ArkErrorCode, ChildId, ThreadId, TurnId};
use ark_ports::{
    EventSink, SessionStore, SubagentPolicy, SubagentPolicySource, SubagentRegistry,
    SubagentRequest,
};
use tokio::sync::Notify;

use super::*;

/// Policy the tests can change between spawns, mirroring a Settings edit.
struct LivePolicy {
    enabled: bool,
    max_concurrency: u32,
    max_queued: u32,
}

impl SubagentPolicySource for LivePolicy {
    fn policy(&self) -> SubagentPolicy {
        SubagentPolicy {
            enabled: self.enabled,
            max_concurrency: self.max_concurrency,
            max_queued: self.max_queued,
        }
    }
}

/// Executor that records what it was asked to do and how many ran concurrently.
struct FakeExecutor {
    summary: String,
    delay: Duration,
    peak_concurrency: AtomicU32,
    in_flight: AtomicU32,
    seen: Mutex<Vec<ChildTurnContext>>,
    fail: bool,
    started: Option<Arc<Notify>>,
    release: Option<Arc<Notify>>,
}

impl FakeExecutor {
    fn new(summary: &str) -> Self {
        Self {
            summary: summary.to_string(),
            delay: Duration::from_millis(5),
            peak_concurrency: AtomicU32::new(0),
            in_flight: AtomicU32::new(0),
            seen: Mutex::new(Vec::new()),
            fail: false,
            started: None,
            release: None,
        }
    }

    fn failing() -> Self {
        let mut executor = Self::new("boom");
        executor.fail = true;
        executor
    }

    fn held(summary: &str, started: Arc<Notify>, release: Arc<Notify>) -> Self {
        let mut executor = Self::new(summary);
        executor.started = Some(started);
        executor.release = Some(release);
        executor
    }

    fn seen(&self) -> Vec<ChildTurnContext> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ChildTurnExecutor for FakeExecutor {
    async fn run_child_turn(&self, ctx: ChildTurnContext) -> Result<ChildTurnReport> {
        let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_concurrency.fetch_max(current, Ordering::SeqCst);
        self.seen.lock().unwrap().push(ctx.clone());
        if let Some(started) = &self.started {
            started.notify_one();
        }
        if let Some(release) = &self.release {
            release.notified().await;
        } else {
            tokio::time::sleep(self.delay).await;
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        if self.fail {
            return Ok(ChildTurnReport::failed("provider 500"));
        }
        let mut report = ChildTurnReport::completed(self.summary.clone());
        report.tool_invocations = 1;
        Ok(report)
    }
}

struct Harness {
    runtime: Arc<DelegationRuntime>,
    store: Arc<ark_store::FileSessionStore>,
    executor: Arc<FakeExecutor>,
    _dir: tempfile::TempDir,
    parent: ThreadId,
    parent_turn: TurnId,
}

/// Build a harness with a real store and a parent thread already created.
///
/// Async on purpose: the parent thread is created through the public
/// `SessionStore` API, so the test exercises the same path production uses.
async fn harness_async(executor: FakeExecutor, policy: (bool, u32, u32)) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ark_store::FileSessionStore::open(ark_store::StoreConfig::new(dir.path())).unwrap(),
    );
    let parent_meta = SessionStore::create_thread(&*store, "parent")
        .await
        .unwrap();
    let parent = parent_meta.thread_id;

    let executor = Arc::new(executor);
    let live = Arc::new(LivePolicy {
        enabled: policy.0,
        max_concurrency: policy.1,
        max_queued: policy.2,
    });
    let sink: Arc<dyn EventSink> = store.clone();
    let store_port: Arc<dyn SessionStore> = store.clone();
    let runtime = Arc::new(DelegationRuntime::new(
        store_port,
        sink,
        live,
        Arc::clone(&executor) as Arc<dyn ChildTurnExecutor>,
    ));
    Harness {
        runtime,
        store,
        executor,
        _dir: dir,
        parent,
        parent_turn: TurnId::new(),
    }
}

impl Harness {
    fn request(&self, prompt: &str) -> SubagentRequest {
        SubagentRequest {
            parent_thread_id: self.parent,
            parent_turn_id: self.parent_turn,
            label: "scan".to_string(),
            prompt: prompt.to_string(),
            ..SubagentRequest::default()
        }
    }

    async fn records(&self) -> Vec<ark_ports::ChildRunRecord> {
        SessionStore::list_child_runs(&*self.store, self.parent)
            .await
            .unwrap()
    }

    /// Latest record for `child` (the log is append-only).
    async fn latest(&self, child: ChildId) -> ark_ports::ChildRunRecord {
        self.records()
            .await
            .into_iter()
            .filter(|record| record.child_id == child)
            .last()
            .expect("child must have at least one record")
    }
}

#[tokio::test]
async fn a_disabled_runtime_refuses_to_spawn() {
    let harness = harness_async(FakeExecutor::new("ok"), (false, 2, 4)).await;
    let err = harness
        .runtime
        .delegate(harness.request("do it"))
        .await
        .unwrap_err();
    assert_eq!(err.code, ArkErrorCode::Conflict);
    assert!(err.message.contains("disabled"), "{}", err.message);
    assert!(
        harness.records().await.is_empty(),
        "no record for a refused spawn"
    );
}

#[tokio::test]
async fn an_empty_prompt_is_a_validation_error() {
    let harness = harness_async(FakeExecutor::new("ok"), (true, 2, 4)).await;
    let err = harness
        .runtime
        .delegate(harness.request("   "))
        .await
        .unwrap_err();
    assert_eq!(err.code, ArkErrorCode::ValidationError);
}

#[tokio::test]
async fn a_successful_run_records_queued_running_and_completed() {
    let harness = harness_async(FakeExecutor::new("the answer"), (true, 2, 4)).await;
    let outcome = harness
        .runtime
        .delegate(harness.request("scan the repo"))
        .await
        .unwrap();

    assert_eq!(outcome.status, ark_ports::child_status::COMPLETED);
    assert_eq!(outcome.summary.as_deref(), Some("the answer"));
    assert_eq!(outcome.tool_invocations, 1);

    let statuses: Vec<String> = harness
        .records()
        .await
        .iter()
        .map(|record| record.status.clone())
        .collect();
    assert_eq!(statuses, vec!["queued", "running", "completed"]);

    let latest = harness.latest(outcome.child_id).await;
    assert_eq!(latest.status, "completed");
    assert!(latest.ended_at.is_some());
    assert_eq!(latest.prompt.as_deref(), Some("scan the repo"));
}

#[tokio::test]
async fn a_child_session_is_inlined_under_the_parent() {
    let harness = harness_async(FakeExecutor::new("ok"), (true, 2, 4)).await;
    let outcome = harness
        .runtime
        .delegate(harness.request("inspect"))
        .await
        .unwrap();

    let paths = harness.store.data_paths();
    let child_paths = paths.child(&harness.parent, &outcome.child_id);
    let child_dir = child_paths.dir();
    assert!(
        child_dir.is_dir(),
        "child directory must exist: {child_dir:?}"
    );
    assert!(
        child_dir.ends_with(
            PathBuf::from(harness.parent.to_string())
                .join("children")
                .join(outcome.child_id.to_string())
        ),
        "child must be nested under the parent: {child_dir:?}"
    );
}

#[tokio::test]
async fn a_failing_child_reports_its_message_and_still_ends() {
    let harness = harness_async(FakeExecutor::failing(), (true, 2, 4)).await;
    let outcome = harness
        .runtime
        .delegate(harness.request("try"))
        .await
        .unwrap();
    assert_eq!(outcome.status, ark_ports::child_status::FAILED);
    assert_eq!(outcome.error.as_deref(), Some("provider 500"));
    assert_eq!(
        SubagentRegistry::active_count(&*harness.runtime),
        0,
        "slot must be released"
    );
}

struct PanickingExecutor;

#[async_trait::async_trait]
impl ChildTurnExecutor for PanickingExecutor {
    async fn run_child_turn(&self, _ctx: ChildTurnContext) -> Result<ChildTurnReport> {
        panic!("simulated child executor panic");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_executor_panic_converges_to_failed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ark_store::FileSessionStore::open(ark_store::StoreConfig::new(dir.path())).unwrap(),
    );
    let parent = SessionStore::create_thread(&*store, "parent")
        .await
        .unwrap()
        .thread_id;
    let runtime = Arc::new(DelegationRuntime::new(
        store.clone() as Arc<dyn SessionStore>,
        store.clone() as Arc<dyn EventSink>,
        Arc::new(LivePolicy {
            enabled: true,
            max_concurrency: 1,
            max_queued: 1,
        }),
        Arc::new(PanickingExecutor),
    ));
    let child = runtime
        .delegate_background(SubagentRequest {
            parent_thread_id: parent,
            parent_turn_id: TurnId::new(),
            label: "panic".to_string(),
            prompt: "panic now".to_string(),
            ..SubagentRequest::default()
        })
        .await
        .unwrap();

    let terminal = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(record) = SubagentRegistry::get(&*runtime, child) {
                if !record.is_active() {
                    break record;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the runtime-owned supervisor must settle a panicking background child");

    assert_eq!(terminal.status, ark_ports::child_status::FAILED);
    assert_eq!(terminal.error.as_deref(), Some("child executor panicked"));
    assert_eq!(SubagentRegistry::active_count(&*runtime), 0);
    assert!(!SubagentRegistry::abort(&*runtime, child));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_concurrency_limit_is_enforced_and_all_runs_finish() {
    let harness = Arc::new(harness_async(FakeExecutor::new("ok"), (true, 2, 8)).await);
    let mut tasks = Vec::new();
    for index in 0..6 {
        let runtime = Arc::clone(&harness.runtime);
        let request = harness.request(&format!("task {index}"));
        tasks.push(tokio::spawn(async move { runtime.delegate(request).await }));
    }
    for task in tasks {
        let outcome = task.await.unwrap().unwrap();
        assert_eq!(outcome.status, ark_ports::child_status::COMPLETED);
    }
    let peak = harness.executor.peak_concurrency.load(Ordering::SeqCst);
    assert!(peak <= 2, "peak {peak} exceeded the limit of 2");
    assert_eq!(SubagentRegistry::active_count(&*harness.runtime), 0);
    assert_eq!(
        harness.records().await.len(),
        6 * 3,
        "three records per run (queued, running, completed)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_saturated_queue_fails_fast_instead_of_hanging() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let harness = Arc::new(
        harness_async(
            FakeExecutor::held("ok", Arc::clone(&started), Arc::clone(&release)),
            (true, 1, 0),
        )
        .await,
    );
    let running = {
        let runtime = Arc::clone(&harness.runtime);
        let request = harness.request("a");
        tokio::spawn(async move { runtime.delegate(request).await })
    };
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("the first child must acquire its slot before queue saturation is asserted");

    let err = harness
        .runtime
        .delegate(harness.request("b"))
        .await
        .unwrap_err();
    assert_eq!(err.code, ArkErrorCode::Conflict);
    assert!(err.message.contains("queue is full"), "{}", err.message);

    release.notify_one();
    running.await.unwrap().unwrap();
}

/// Executor that waits for its cancel token, so an abort test proves the signal.
struct BlockingExecutor {
    seen: Mutex<Vec<Arc<dyn ark_ports::CancelHandle>>>,
}

#[async_trait::async_trait]
impl ChildTurnExecutor for BlockingExecutor {
    async fn run_child_turn(&self, ctx: ChildTurnContext) -> Result<ChildTurnReport> {
        self.seen.lock().unwrap().push(Arc::clone(&ctx.cancel));
        for _ in 0..200 {
            if ctx.cancel.is_cancelled() {
                return Ok(ChildTurnReport::aborted("stopped by user"));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(ChildTurnReport::completed("never aborted"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_reaches_the_running_child() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ark_store::FileSessionStore::open(ark_store::StoreConfig::new(dir.path())).unwrap(),
    );
    let parent = SessionStore::create_thread(&*store, "parent")
        .await
        .unwrap()
        .thread_id;
    let executor = Arc::new(BlockingExecutor {
        seen: Mutex::new(Vec::new()),
    });
    let live = Arc::new(LivePolicy {
        enabled: true,
        max_concurrency: 1,
        max_queued: 2,
    });
    let runtime = Arc::new(DelegationRuntime::new(
        store.clone() as Arc<dyn SessionStore>,
        store.clone() as Arc<dyn EventSink>,
        live,
        Arc::clone(&executor) as Arc<dyn ChildTurnExecutor>,
    ));

    let request = SubagentRequest {
        parent_thread_id: parent,
        parent_turn_id: TurnId::new(),
        label: "long".to_string(),
        prompt: "wait forever".to_string(),
        ..SubagentRequest::default()
    };
    let prepared = runtime.prepare(request).await.unwrap();
    let child = prepared.record.child_id;
    let running = {
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move { runtime.execute(prepared).await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;

    assert!(
        SubagentRegistry::abort(&*runtime, child),
        "a running child must be abortable"
    );
    let outcome = running.await.unwrap().unwrap();
    assert_eq!(outcome.status, ark_ports::child_status::ABORTED);
    assert!(
        !SubagentRegistry::abort(&*runtime, child),
        "an ended child is no longer abortable"
    );
}

#[tokio::test]
async fn a_completed_run_cannot_be_resumed() {
    let harness = harness_async(FakeExecutor::new("ok"), (true, 2, 4)).await;
    let outcome = harness
        .runtime
        .delegate(harness.request("once"))
        .await
        .unwrap();

    let mut request = harness.request("again");
    request.resume_child_id = Some(outcome.child_id);
    let err = harness.runtime.delegate(request).await.unwrap_err();
    assert_eq!(err.code, ArkErrorCode::Conflict);
    assert!(err.message.contains("already completed"), "{}", err.message);
}

#[tokio::test]
async fn a_failed_run_can_be_resumed_and_counts_resumes() {
    let harness = harness_async(FakeExecutor::failing(), (true, 2, 4)).await;
    let failed = harness
        .runtime
        .delegate(harness.request("first try"))
        .await
        .unwrap();
    assert_eq!(failed.status, ark_ports::child_status::FAILED);

    let mut request = harness.request("corrected instruction");
    request.resume_child_id = Some(failed.child_id);
    let resumed = harness.runtime.delegate(request).await.unwrap();
    assert_eq!(
        resumed.child_id, failed.child_id,
        "same session, same child id"
    );

    let latest = harness.latest(failed.child_id).await;
    assert_eq!(latest.resume_count, Some(1));
}

#[tokio::test]
async fn a_child_of_a_child_is_refused() {
    // v1 supports exactly one level; the depth policy lives in the store and must
    // surface through delegation rather than be re-implemented here.
    let harness = harness_async(FakeExecutor::new("ok"), (true, 2, 4)).await;
    let first = harness
        .runtime
        .delegate(harness.request("level 1"))
        .await
        .unwrap();

    let child_thread = ThreadId::from_uuid(first.child_id.as_uuid());
    let nested = SubagentRequest {
        parent_thread_id: child_thread,
        parent_turn_id: TurnId::new(),
        label: "level 2".to_string(),
        prompt: "nested".to_string(),
        ..SubagentRequest::default()
    };
    let err = harness.runtime.delegate(nested).await.unwrap_err();
    assert!(
        err.message.contains("depth")
            || err.message.contains("child")
            || err.code == ArkErrorCode::ValidationError,
        "expected a depth refusal, got {:?} / {}",
        err.code,
        err.message
    );
}

#[tokio::test]
async fn hydrate_converges_an_orphaned_run_instead_of_leaving_it_active() {
    let harness = harness_async(FakeExecutor::new("ok"), (true, 2, 4)).await;
    // Simulate a process death: a durable `queued` row with no live task behind
    // it. Hydration is the only place allowed to repair that.
    let child = ChildId::new();
    let orphan = ark_ports::ChildRunRecord::queued(
        child,
        harness.parent,
        harness.parent_turn,
        "orphan",
        chrono::Utc::now(),
    );
    SessionStore::append_child_run(&*harness.store, orphan)
        .await
        .unwrap();

    let loaded = harness.runtime.hydrate().await.unwrap();
    assert_eq!(loaded, 1);

    let record = SubagentRegistry::get(&*harness.runtime, child).expect("child must be hydrated");
    assert_eq!(record.status, ark_ports::child_status::FAILED);
    assert!(
        !record.is_active(),
        "a hydrated orphan must not stay active"
    );
    assert_eq!(
        record.error.as_deref(),
        Some("interrupted by a runtime restart")
    );
    assert!(!SubagentRegistry::abort(&*harness.runtime, child));
}

#[tokio::test]
async fn registry_lists_and_finds_runs() {
    let harness = harness_async(FakeExecutor::new("ok"), (true, 2, 4)).await;
    let first = harness
        .runtime
        .delegate(harness.request("one"))
        .await
        .unwrap();
    let second = harness
        .runtime
        .delegate(harness.request("two"))
        .await
        .unwrap();

    let listed = SubagentRegistry::list(&*harness.runtime, Some(harness.parent));
    assert_eq!(listed.len(), 2, "one latest record per child");
    let found = SubagentRegistry::get(&*harness.runtime, second.child_id).expect("second run");
    assert_eq!(found.prompt.as_deref(), Some("two"));
    assert_eq!(
        SubagentRegistry::get(&*harness.runtime, ChildId::new()),
        None
    );
    assert_eq!(SubagentRegistry::list(&*harness.runtime, None).len(), 2);
    let _ = first;
}

#[tokio::test]
async fn the_policy_is_read_per_spawn_so_a_settings_change_applies() {
    // The runtime must not cache the limit: `subagents` is a Hot config field.
    struct TogglePolicy {
        enabled: AtomicU32,
    }
    impl SubagentPolicySource for TogglePolicy {
        fn policy(&self) -> SubagentPolicy {
            SubagentPolicy {
                enabled: self.enabled.load(Ordering::SeqCst) == 1,
                max_concurrency: 1,
                max_queued: 0,
            }
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ark_store::FileSessionStore::open(ark_store::StoreConfig::new(dir.path())).unwrap(),
    );
    let parent = SessionStore::create_thread(&*store, "p")
        .await
        .unwrap()
        .thread_id;
    let policy = Arc::new(TogglePolicy {
        enabled: AtomicU32::new(0),
    });
    let runtime = Arc::new(DelegationRuntime::new(
        store.clone() as Arc<dyn SessionStore>,
        store.clone() as Arc<dyn EventSink>,
        Arc::clone(&policy) as Arc<dyn SubagentPolicySource>,
        Arc::new(FakeExecutor::new("ok")) as Arc<dyn ChildTurnExecutor>,
    ));

    let request = SubagentRequest {
        parent_thread_id: parent,
        parent_turn_id: TurnId::new(),
        label: "x".to_string(),
        prompt: "y".to_string(),
        ..SubagentRequest::default()
    };
    assert!(runtime.delegate(request.clone()).await.is_err());

    policy.enabled.store(1, Ordering::SeqCst);
    assert_eq!(
        runtime.delegate(request).await.unwrap().status,
        "completed",
        "a settings change must apply to the next spawn without a restart"
    );
}

/// Executor that drains its steering queue once anything is queued.
struct SteerAware {
    delivered: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ChildTurnExecutor for SteerAware {
    async fn run_child_turn(&self, ctx: ChildTurnContext) -> Result<ChildTurnReport> {
        for _ in 0..100 {
            if ctx.steer.len() > 0 {
                let queued = ctx.steer.drain();
                self.delivered.lock().unwrap().extend(queued);
                return Ok(ChildTurnReport::completed("steered"));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(ChildTurnReport::completed("no steer"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steering_reaches_a_running_child() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ark_store::FileSessionStore::open(ark_store::StoreConfig::new(dir.path())).unwrap(),
    );
    let parent = SessionStore::create_thread(&*store, "p")
        .await
        .unwrap()
        .thread_id;
    let executor = Arc::new(SteerAware {
        delivered: Mutex::new(Vec::new()),
    });
    let runtime = Arc::new(DelegationRuntime::new(
        store.clone() as Arc<dyn SessionStore>,
        store.clone() as Arc<dyn EventSink>,
        Arc::new(LivePolicy {
            enabled: true,
            max_concurrency: 1,
            max_queued: 2,
        }),
        Arc::clone(&executor) as Arc<dyn ChildTurnExecutor>,
    ));

    let prepared = runtime
        .prepare(SubagentRequest {
            parent_thread_id: parent,
            parent_turn_id: TurnId::new(),
            label: "s".to_string(),
            prompt: "go".to_string(),
            ..SubagentRequest::default()
        })
        .await
        .unwrap();
    let child = prepared.record.child_id;
    let running = {
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move { runtime.execute(prepared).await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;

    assert!(
        SubagentRegistry::steer(&*runtime, child, "be more careful"),
        "steer must be accepted for a running child"
    );
    let outcome = running.await.unwrap().unwrap();
    assert_eq!(outcome.status, "completed");
    assert_eq!(
        executor.delivered.lock().unwrap().as_slice(),
        ["be more careful".to_string()]
    );
}

#[tokio::test]
async fn child_activity_events_are_published_on_the_parent_stream() {
    let harness = harness_async(FakeExecutor::new("done"), (true, 2, 4)).await;
    let outcome = harness
        .runtime
        .delegate(harness.request("trace me"))
        .await
        .unwrap();

    let events = SessionStore::replay_events(&*harness.store, harness.parent, 0)
        .await
        .unwrap();
    let activities: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            ark_contract::RuntimeEvent::ChildActivity {
                child_id, status, ..
            } if *child_id == outcome.child_id => Some(status.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        activities,
        vec!["queued", "running", "completed"],
        "the parent stream must carry the child lifecycle"
    );
}

#[tokio::test]
async fn a_background_child_publishes_a_terminal_completion_notification() {
    let harness = harness_async(FakeExecutor::new("final report"), (true, 2, 4)).await;
    let child = harness
        .runtime
        .delegate_background(harness.request("research asynchronously"))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !harness.latest(child).await.is_active() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the detached child must reach a terminal record");

    let events = SessionStore::replay_events(&*harness.store, harness.parent, 0)
        .await
        .unwrap();
    let completion = events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .find(|event| event["kind"] == "child_completed")
        .expect("a terminal child must notify the parent thread");

    assert_eq!(completion["child_id"], child.to_string());
    assert_eq!(completion["summary"], "final report");
    assert_eq!(completion["status"], ark_ports::child_status::COMPLETED);
}

#[tokio::test]
async fn hydrate_replays_a_missing_terminal_completion_notification() {
    let harness = harness_async(FakeExecutor::new("unused"), (true, 2, 4)).await;
    let child = ChildId::new();
    let mut terminal = ark_ports::ChildRunRecord::queued(
        child,
        harness.parent,
        harness.parent_turn,
        "recovered",
        chrono::Utc::now(),
    );
    terminal.status = ark_ports::child_status::COMPLETED.to_string();
    terminal.ended_at = Some(chrono::Utc::now());
    terminal.summary = Some("recovered report".to_string());
    SessionStore::append_child_run(&*harness.store, terminal)
        .await
        .unwrap();

    harness.runtime.hydrate().await.unwrap();

    let events = SessionStore::replay_events(&*harness.store, harness.parent, 0)
        .await
        .unwrap();
    let completions = events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .filter(|event| event["kind"] == "child_completed")
        .collect::<Vec<_>>();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0]["child_id"], child.to_string());
    assert_eq!(completions[0]["summary"], "recovered report");

    harness.runtime.hydrate().await.unwrap();
    let events = SessionStore::replay_events(&*harness.store, harness.parent, 0)
        .await
        .unwrap();
    let replayed = events
        .iter()
        .filter(|event| event.kind().as_str() == "child_completed")
        .count();
    assert_eq!(
        replayed, 1,
        "hydrate must not duplicate a delivered notification"
    );
}

#[tokio::test]
async fn a_finished_run_frees_its_slot_and_reports_not_active() {
    let harness = harness_async(FakeExecutor::new("ok"), (true, 1, 1)).await;
    for index in 0..3 {
        let outcome = harness
            .runtime
            .delegate(harness.request(&format!("seq {index}")))
            .await
            .unwrap();
        assert_eq!(outcome.status, "completed");
        assert_eq!(
            SubagentRegistry::active_count(&*harness.runtime),
            0,
            "finished runs free the slot"
        );
    }
    let records = harness.records().await;
    assert_eq!(records.len(), 9, "three records per run");

    // The log is append-only, so the `queued`/`running` rows legitimately carry
    // no `ended_at`. What must hold is that the *latest* row per child is
    // terminal, which is what the index projection and the UI read.
    let mut latest: std::collections::HashMap<ChildId, ark_ports::ChildRunRecord> =
        std::collections::HashMap::new();
    for record in records {
        latest.insert(record.child_id, record);
    }
    assert_eq!(latest.len(), 3, "three distinct children");
    assert!(
        latest.values().all(|record| !record.is_active()),
        "no child may stay active after its run finished: {:?}",
        latest
            .values()
            .filter(|record| record.is_active())
            .map(|record| (record.child_id, record.status.clone()))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn the_executor_receives_the_prompt_and_parent_identity() {
    let harness = harness_async(FakeExecutor::new("ok"), (true, 2, 4)).await;
    harness
        .runtime
        .delegate(harness.request("explain the build"))
        .await
        .unwrap();
    let seen = harness.executor.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].prompt, "explain the build");
    assert_eq!(seen[0].parent_thread_id, harness.parent);
    assert_eq!(seen[0].parent_turn_id, harness.parent_turn);
}
