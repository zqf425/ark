use super::*;

struct FaultHarness {
    state: AppState,
    runner: Arc<RecordingRunner>,
    store: Arc<RuntimeFaultStore>,
    schedules: Arc<FaultyScheduleStore>,
    _dir: tempfile::TempDir,
}

fn fault_harness(runtime_fail: bool, fault: ScheduleFault) -> FaultHarness {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().to_path_buf();
    let inner = Arc::new(FileSessionStore::open(StoreConfig::new(&root)).expect("store"));
    let store = Arc::new(RuntimeFaultStore {
        inner: Arc::clone(&inner),
        fail_runtime: runtime_fail,
        created: StdMutex::new(Vec::new()),
        deleted: AtomicUsize::new(0),
    });
    let schedules = Arc::new(FaultyScheduleStore {
        inner: ScheduleFileStore::open(&root).expect("schedule store"),
        fault,
    });
    let runner = Arc::new(RecordingRunner {
        store: inner,
        prompts: StdMutex::new(Vec::new()),
        fail: false,
        gate: None,
    });
    let state = AppState::new(
        store.clone(),
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig::default())),
        Arc::new(UsageRecorder::new()),
        runner.clone(),
        root,
        "test",
    );
    FaultHarness {
        state,
        runner,
        store,
        schedules,
        _dir: dir,
    }
}

async fn assert_orphan_cleaned(harness: &FaultHarness) {
    let thread_id = {
        let created = harness.store.created.lock().unwrap();
        assert_eq!(created.len(), 1);
        created[0]
    };
    assert_eq!(harness.store.deleted.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness
            .store
            .inner
            .get_thread(thread_id)
            .await
            .unwrap_err()
            .code,
        ArkErrorCode::NotFound
    );
}

#[tokio::test]
async fn runtime_write_failure_compensates_the_created_thread() {
    let harness = fault_harness(true, ScheduleFault::None);
    let task = due_task(0);
    harness.schedules.inner.insert(task.clone()).unwrap();
    let service = SchedulerService::new(harness.schedules.clone());

    let error = service.begin(&harness.state, task).await.unwrap_err();

    assert_eq!(error.code, ArkErrorCode::StoreFailure);
    assert_orphan_cleaned(&harness).await;
    assert!(harness
        .schedules
        .inner
        .get("sch_test")
        .unwrap()
        .last_run
        .is_none());
}

#[tokio::test]
async fn running_marker_failure_compensates_the_created_thread() {
    let harness = fault_harness(false, ScheduleFault::MarkerFailure);
    let task = due_task(0);
    harness.schedules.inner.insert(task.clone()).unwrap();
    let service = SchedulerService::new(harness.schedules.clone());

    let error = service.begin(&harness.state, task).await.unwrap_err();

    assert_eq!(error.code, ArkErrorCode::StoreFailure);
    assert_orphan_cleaned(&harness).await;
    assert!(harness
        .schedules
        .inner
        .get("sch_test")
        .unwrap()
        .last_run
        .is_none());
}

#[tokio::test]
async fn interleaved_execution_patch_abandons_the_run_and_cleans_its_thread() {
    let harness = fault_harness(false, ScheduleFault::PatchBeforeMarker);
    let task = due_task(0);
    harness.schedules.inner.insert(task.clone()).unwrap();
    let service = SchedulerService::new(harness.schedules.clone());

    let error = service.begin(&harness.state, task).await.unwrap_err();

    assert_eq!(error.code, ArkErrorCode::Conflict);
    assert_orphan_cleaned(&harness).await;
    let stored = harness.schedules.inner.get("sch_test").unwrap();
    assert_eq!(stored.prompt, "patched prompt");
    assert!(stored.last_run.is_none());
    assert!(harness.runner.prompts.lock().unwrap().is_empty());
}

#[test]
fn execution_snapshot_captures_every_execution_field_and_default_autonomy() {
    let mut task = due_task(0);
    task.workspace = Some("/tmp".into());
    task.provider_id = Some("provider-old".into());
    task.model = Some("model-old".into());

    let snapshot = ExecutionSnapshot::from_task(&task);

    assert_eq!(snapshot.prompt, task.prompt);
    assert_eq!(snapshot.workspace, task.workspace);
    assert_eq!(snapshot.provider_id, task.provider_id);
    assert_eq!(snapshot.model, task.model);
    assert_eq!(snapshot.autonomy, DEFAULT_SCHEDULE_AUTONOMY);
}

#[tokio::test]
async fn runtime_and_prompt_use_the_same_precreate_snapshot() {
    let harness = Harness::new(true, false, None);
    let mut task = due_task(0);
    task.workspace = Some("/tmp".into());
    task.autonomy = Some("workspace_write".into());
    task.provider_id = None;
    task.model = None;
    harness.seed(task.clone());

    let (mut stored, thread_id, started_at, snapshot) = harness
        .service
        .begin(&harness.state, task)
        .await
        .expect("begin with immutable snapshot");
    assert_eq!(snapshot.prompt, "do the thing");
    assert_eq!(snapshot.workspace.as_deref(), Some("/tmp"));
    assert_eq!(snapshot.provider_id, None);
    assert_eq!(snapshot.model, None);
    assert_eq!(snapshot.autonomy, "workspace_write");
    stored.prompt = "patched prompt".into();

    harness
        .service
        .drive(&harness.state, stored, thread_id, started_at, snapshot)
        .await;

    let runtime = harness.store.get_thread(thread_id).await.unwrap().runtime;
    assert_eq!(runtime.workspace.as_deref(), Some("/tmp"));
    assert_eq!(runtime.provider_id, None);
    assert_eq!(runtime.model, None);
    assert_eq!(runtime.autonomy.as_deref(), Some("workspace_write"));
    assert_eq!(
        harness
            .runner
            .prompts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
        vec!["do the thing"]
    );
}
