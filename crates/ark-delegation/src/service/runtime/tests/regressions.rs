use std::sync::atomic::AtomicBool;

use ark_ports::Subscription;

use super::*;

struct CompletionGateSink {
    inner: Arc<ark_store::FileSessionStore>,
    block: AtomicBool,
    entered: Notify,
    release: Notify,
}

impl CompletionGateSink {
    fn new(inner: Arc<ark_store::FileSessionStore>) -> Self {
        Self {
            inner,
            block: AtomicBool::new(true),
            entered: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl EventSink for CompletionGateSink {
    async fn publish(
        &self,
        thread: ThreadId,
        event: RuntimeEvent,
    ) -> ark_common::Result<RuntimeEvent> {
        if matches!(event, RuntimeEvent::ChildCompleted { .. })
            && self.block.swap(false, Ordering::SeqCst)
        {
            self.entered.notify_one();
            self.release.notified().await;
        }
        EventSink::publish(&*self.inner, thread, event).await
    }

    async fn subscribe(
        &self,
        thread: ThreadId,
        since_seq: u64,
    ) -> ark_common::Result<Subscription> {
        EventSink::subscribe(&*self.inner, thread, since_seq).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_run_cleanup_does_not_remove_a_resumed_runs_handle() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ark_store::FileSessionStore::open(ark_store::StoreConfig::new(dir.path())).unwrap(),
    );
    let parent = SessionStore::create_thread(&*store, "parent")
        .await
        .unwrap()
        .thread_id;
    let child = ChildId::new();
    let mut failed = ark_ports::ChildRunRecord::queued(
        child,
        parent,
        TurnId::new(),
        "resumable",
        chrono::Utc::now(),
    );
    failed.status = ark_common::ChildRunStatus::Failed;
    failed.ended_at = Some(chrono::Utc::now());
    failed.error = Some("first attempt failed".to_string());
    failed.notify_parent = false;
    SessionStore::append_child_run(&*store, failed)
        .await
        .unwrap();

    let sink = Arc::new(CompletionGateSink::new(Arc::clone(&store)));
    let runtime = Arc::new(DelegationRuntime::new(
        store.clone() as Arc<dyn SessionStore>,
        sink.clone() as Arc<dyn EventSink>,
        Arc::new(LivePolicy {
            enabled: true,
            max_concurrency: 1,
            max_queued: 1,
        }),
        Arc::new(FakeExecutor::failing()),
    ));
    runtime.hydrate().await.unwrap();
    let mut request = SubagentRequest {
        parent_thread_id: parent,
        parent_turn_id: TurnId::new(),
        label: "resumable".to_string(),
        prompt: "try again".to_string(),
        resume_child_id: Some(child),
        ..SubagentRequest::default()
    };
    runtime.prepare_child(request.clone()).await.unwrap();
    let worker_runtime = Arc::clone(&runtime);
    let running = tokio::spawn(async move { worker_runtime.run_child(child).await });
    tokio::time::timeout(Duration::from_secs(2), sink.entered.notified())
        .await
        .expect("the first resume must reach completion publication");

    request.prompt = "newer resume".to_string();
    runtime.prepare_child(request).await.unwrap();
    sink.release.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .expect("the first resume must finish")
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, ark_common::ChildRunStatus::Failed);
    assert!(
        SubagentRegistry::abort(&*runtime, child),
        "the newer resume's control handle must survive the old RunSlot cleanup"
    );
    runtime.abort_all().await;
}

#[tokio::test]
async fn resume_persists_the_effective_configuration_overrides() {
    let harness = harness_async(FakeExecutor::failing(), (true, 2, 4)).await;
    let failed = harness
        .runtime
        .delegate(harness.request("first try"))
        .await
        .unwrap();
    assert_eq!(failed.status, ark_common::ChildRunStatus::Failed);

    let mut request = harness.request("corrected instruction");
    request.resume_child_id = Some(failed.child_id);
    request.model = Some("resumed-model".to_string());
    request.provider_id = Some("resumed-provider".to_string());
    request.autonomy = Some("unattended".to_string());
    let resumed = harness.runtime.delegate(request).await.unwrap();

    assert_eq!(resumed.model.as_deref(), Some("resumed-model"));
    assert_eq!(resumed.provider_id.as_deref(), Some("resumed-provider"));
    let latest = harness.latest(failed.child_id).await;
    assert_eq!(latest.model.as_deref(), Some("resumed-model"));
    assert_eq!(latest.provider_id.as_deref(), Some("resumed-provider"));
    assert_eq!(latest.autonomy.as_deref(), Some("unattended"));
    assert_eq!(latest.resume_count, Some(1));
}
