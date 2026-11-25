use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use ark_common::{ChildId, Result, ThreadId, TurnId};
use ark_contract::{RewindResponse, RuntimeEvent, ThreadItem, ThreadRuntimeSettings};
use ark_ports::store::{ChildRunRecord, SessionStore, SubscribeTicket, ThreadMeta};
use async_trait::async_trait;
use tokio::sync::Notify;

pub struct StorePause {
    enabled: AtomicBool,
    pub entered: Notify,
    pub release: Notify,
}

impl StorePause {
    fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            entered: Notify::new(),
            release: Notify::new(),
        }
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    async fn wait(&self) {
        if self.enabled.load(Ordering::Acquire) {
            self.entered.notify_one();
            self.release.notified().await;
        }
    }

    pub fn resume(&self) {
        self.release.notify_one();
    }
}

pub struct GatedStore {
    inner: Arc<dyn SessionStore>,
    pub replay: StorePause,
    pub interaction: StorePause,
    pub list: StorePause,
    pub delete: StorePause,
    fail_append_at: AtomicUsize,
    append_attempts: AtomicUsize,
    pub append_failed: Notify,
}

impl GatedStore {
    pub fn new(inner: Arc<dyn SessionStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            replay: StorePause::new(),
            interaction: StorePause::new(),
            list: StorePause::new(),
            delete: StorePause::new(),
            fail_append_at: AtomicUsize::new(0),
            append_attempts: AtomicUsize::new(0),
            append_failed: Notify::new(),
        })
    }

    pub fn fail_append_at(&self, attempt: usize) {
        self.fail_append_at.store(attempt, Ordering::Release);
    }
}

#[async_trait]
impl SessionStore for GatedStore {
    async fn create_thread(&self, title: &str) -> Result<ThreadMeta> {
        self.inner.create_thread(title).await
    }

    async fn list_threads(&self) -> Result<Vec<ThreadMeta>> {
        self.list.wait().await;
        self.inner.list_threads().await
    }

    async fn get_thread(&self, thread: ThreadId) -> Result<ThreadMeta> {
        self.inner.get_thread(thread).await
    }

    async fn rename_thread(&self, thread: ThreadId, title: &str) -> Result<ThreadMeta> {
        self.inner.rename_thread(thread, title).await
    }

    async fn set_archived(&self, thread: ThreadId, archived: bool) -> Result<ThreadMeta> {
        self.inner.set_archived(thread, archived).await
    }

    async fn set_thread_runtime(
        &self,
        thread: ThreadId,
        runtime: ThreadRuntimeSettings,
    ) -> Result<ThreadMeta> {
        self.inner.set_thread_runtime(thread, runtime).await
    }

    async fn delete_thread(&self, thread: ThreadId) -> Result<()> {
        self.delete.wait().await;
        self.inner.delete_thread(thread).await
    }

    async fn append_event(&self, thread: ThreadId, event: RuntimeEvent) -> Result<RuntimeEvent> {
        let attempt = self.append_attempts.fetch_add(1, Ordering::AcqRel) + 1;
        let fail_at = self.fail_append_at.load(Ordering::Acquire);
        if fail_at != 0 && attempt >= fail_at {
            self.append_failed.notify_one();
            return Err(ark_common::ArkError::internal("injected append failure"));
        }
        self.inner.append_event(thread, event).await
    }

    async fn replay_events(&self, thread: ThreadId, since_seq: u64) -> Result<Vec<RuntimeEvent>> {
        self.replay.wait().await;
        self.inner.replay_events(thread, since_seq).await
    }

    async fn replay_child_completion_events(&self, thread: ThreadId) -> Result<Vec<RuntimeEvent>> {
        self.inner.replay_child_completion_events(thread).await
    }

    async fn replay_interaction_events(&self, thread: ThreadId) -> Result<Vec<RuntimeEvent>> {
        self.interaction.wait().await;
        self.inner.replay_interaction_events(thread).await
    }

    async fn replay_attachment_events(&self, thread: ThreadId) -> Result<Vec<RuntimeEvent>> {
        self.inner.replay_attachment_events(thread).await
    }

    async fn highest_seq(&self, thread: ThreadId) -> Result<u64> {
        self.inner.highest_seq(thread).await
    }

    async fn read_items(&self, thread: ThreadId) -> Result<Vec<ThreadItem>> {
        self.inner.read_items(thread).await
    }

    async fn subscribe(&self, thread: ThreadId, since_seq: u64) -> Result<SubscribeTicket> {
        self.inner.subscribe(thread, since_seq).await
    }

    async fn fork_thread(&self, thread: ThreadId, upto_seq: u64) -> Result<ThreadMeta> {
        self.inner.fork_thread(thread, upto_seq).await
    }

    async fn rewind_thread(
        &self,
        thread: ThreadId,
        rewind_from_seq: u64,
        target_turn_id: Option<TurnId>,
    ) -> Result<RewindResponse> {
        self.inner
            .rewind_thread(thread, rewind_from_seq, target_turn_id)
            .await
    }

    async fn rewind_record(
        &self,
        thread: ThreadId,
        rewind_from_seq: Option<u64>,
        target_turn_id: Option<TurnId>,
    ) -> Result<Option<RewindResponse>> {
        self.inner
            .rewind_record(thread, rewind_from_seq, target_turn_id)
            .await
    }

    async fn create_child_run(&self, parent: ThreadId, child: ChildId, label: &str) -> Result<()> {
        self.inner.create_child_run(parent, child, label).await
    }

    async fn append_child_run(&self, record: ChildRunRecord) -> Result<()> {
        self.inner.append_child_run(record).await
    }

    async fn list_child_runs(&self, parent: ThreadId) -> Result<Vec<ChildRunRecord>> {
        self.inner.list_child_runs(parent).await
    }

    async fn delete_child_run(&self, parent: ThreadId, child: ChildId) -> Result<bool> {
        self.inner.delete_child_run(parent, child).await
    }
}
