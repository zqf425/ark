//! Thread lifecycle and the `SessionStore` / `EventSink` implementations.
//!
//! Every event operation goes through the per-thread writer actor, so `seq`
//! allocation, the durable append and the subscribe watermark all share one
//! serialization point (design 7.4.3).

use crate::domain::layout::ThreadPaths;
use crate::domain::thread_file::ThreadFile;
use crate::service::store::FileSessionStore;
use crate::store::child_store::rebuild_child_index_blocking;
use crate::store::event_log::read_markers_blocking;
use crate::store::index::IndexEntry;
use crate::store::items_log::{read_item_snapshots_blocking, write_items_blocking, UsageRecord};
use ark_common::{ArkError, Result, ThreadId};
use ark_contract::{RuntimeEvent, ThreadItem, ThreadSummary};
use ark_ports::{ChildRunRecord, SessionStore, SubscribeTicket, ThreadMeta};

impl FileSessionStore {
    /// Create the thread directory, the metadata file and the index row.
    pub(crate) fn create_thread_blocking(&self, title: &str) -> Result<ThreadMeta> {
        let file = ThreadFile::new(ThreadId::new(), title);
        self.create_thread_dir(&file)?;
        self.index_upsert(&file, 0)?;
        Ok(file.to_meta())
    }

    /// Threads from the index, active first then most recently updated.
    pub(crate) fn list_threads_blocking(&self) -> Result<Vec<ThreadMeta>> {
        let index = self.lock_index()?;
        Ok(index.sorted().iter().map(entry_to_meta).collect())
    }

    pub(crate) fn rename_thread_blocking(
        &self,
        thread: ThreadId,
        title: &str,
    ) -> Result<ThreadMeta> {
        let mut file = self.require_thread_file(thread)?;
        file.title = crate::domain::thread_file::normalize_title(title);
        file.touch();
        crate::store::fs_util::write_json_atomic(&self.thread_paths(thread).thread_json(), &file)?;
        self.index_upsert(&file, self.highest_seq_of(thread))?;
        Ok(file.to_meta())
    }

    pub(crate) fn set_archived_blocking(
        &self,
        thread: ThreadId,
        archived: bool,
    ) -> Result<ThreadMeta> {
        let mut file = self.require_thread_file(thread)?;
        file.archived = archived;
        file.touch();
        crate::store::fs_util::write_json_atomic(&self.thread_paths(thread).thread_json(), &file)?;
        self.index_upsert(&file, self.highest_seq_of(thread))?;
        Ok(file.to_meta())
    }

    /// Highest durable `seq` read straight from the log (no writer needed).
    pub(crate) fn highest_seq_of(&self, thread: ThreadId) -> u64 {
        crate::store::event_log::read_log_blocking(&self.thread_paths(thread))
            .map(|parsed| parsed.max_seq)
            .unwrap_or(0)
    }

    /// Rebuild `items.jsonl` from the event log.
    pub(crate) fn rebuild_items(&self, thread: ThreadId) -> Result<Vec<ThreadItem>> {
        crate::store::items_log::rebuild_items_blocking(&self.thread_paths(thread))
    }

    /// Write a forked copy of `(0, upto_seq]` into a brand-new thread directory.
    pub(crate) fn fork_blocking(&self, thread: ThreadId, upto_seq: u64) -> Result<ThreadMeta> {
        let source = self.require_thread_file(thread)?;
        let events: Vec<RuntimeEvent> =
            crate::store::event_log::read_log_blocking(&self.thread_paths(thread))?
                .events
                .into_iter()
                .filter(|event| event.seq() <= upto_seq)
                .collect();
        if events.is_empty() && upto_seq > 0 {
            return Err(ArkError::validation(format!(
                "cannot fork thread {thread} at seq {upto_seq}: no durable events at or below it"
            )));
        }
        let target = ThreadId::new();
        let forked = source.forked_from(target);
        let paths = self.create_thread_dir(&forked)?;
        let mut body = String::new();
        for event in &events {
            let retargeted = crate::domain::retarget::retarget_event(event, target);
            body.push_str(&crate::store::event_log::encode_event(&retargeted)?);
        }
        crate::store::fs_util::write_atomic(&paths.events_jsonl(), body.as_bytes())?;
        let items = crate::store::items_log::rebuild_items_blocking(&paths)?;
        let _ = items;
        self.index_upsert(&forked, upto_seq)?;
        Ok(forked.to_meta())
    }

    /// Rewrite the derived files for a thread from its event log.
    pub(crate) fn repair_derived(&self, thread: ThreadId) -> Result<()> {
        let paths = self.thread_paths(thread);
        let events = crate::store::event_log::read_log_blocking(&paths)?.events;
        write_items_blocking(&paths, &crate::domain::items::project_items(&events))?;
        let session = crate::domain::session_state::SessionState::replay(&events);
        crate::store::fs_util::write_json_atomic(&paths.session_json(), &session)?;
        let mut usage = ark_common::Usage::default();
        for record in crate::store::items_log::read_usage_blocking(&paths) {
            usage.merge(&record.usage);
        }
        Ok(())
    }

    /// Append a child-run record and refresh the derived child index.
    ///
    /// The authoritative append, index rebuild and locator update are one
    /// parent-scoped transaction. This shares the same lock as delete's
    /// read/modify/write, so deleting one child cannot overwrite another
    /// child's concurrent status append.
    ///
    /// The two writes are deliberately **not** treated as equally fatal: the log
    /// line is the system of record, while `index.json` is a cache. Reporting a
    /// landed append as failed made the delegation runtime believe a stored
    /// terminal row had not been written and keep the child `running` in memory
    /// until the next restart (P1-3).
    pub(crate) fn append_child_run_blocking(&self, record: &ChildRunRecord) -> Result<()> {
        self.with_child_log_lock(record.parent_thread_id, || {
            let parent_paths = self.thread_paths(record.parent_thread_id);
            crate::store::child_store::append_child_run_blocking(
                &parent_paths,
                record,
                self.config.limits().fsync_on_append,
            )?;
            // Cache only: a failure here is reported, never propagated, because
            // the next read or reconcile rebuilds it from the log.
            if let Err(err) = rebuild_child_index_blocking(&parent_paths) {
                tracing::warn!(
                    parent = %record.parent_thread_id,
                    child = %record.child_id,
                    error = %err.message,
                    "child-run record stored but its derived index could not be rebuilt"
                );
            }
            self.remember_child(record.parent_thread_id, std::slice::from_ref(record))
        })
    }
}

fn entry_to_meta(entry: &IndexEntry) -> ThreadMeta {
    ThreadMeta {
        thread_id: entry.thread_id,
        title: entry.title.clone(),
        created_at: entry.updated_at,
        updated_at: entry.updated_at,
        archived: entry.archived,
    }
}

#[async_trait::async_trait]
impl SessionStore for FileSessionStore {
    async fn create_thread(&self, title: &str) -> Result<ThreadMeta> {
        self.create_thread_blocking(title)
    }

    async fn list_threads(&self) -> Result<Vec<ThreadMeta>> {
        self.list_threads_blocking()
    }

    async fn get_thread(&self, thread: ThreadId) -> Result<ThreadMeta> {
        Ok(self.require_thread_file(thread)?.to_meta())
    }

    async fn rename_thread(&self, thread: ThreadId, title: &str) -> Result<ThreadMeta> {
        self.rename_thread_blocking(thread, title)
    }

    async fn set_archived(&self, thread: ThreadId, archived: bool) -> Result<ThreadMeta> {
        self.set_archived_blocking(thread, archived)
    }

    async fn delete_thread(&self, thread: ThreadId) -> Result<()> {
        self.require_thread_file(thread)?;
        self.ensure_no_active_children(thread)?;
        let tombstone = self.child_store(thread).tombstone_children();
        self.remove_thread_dir(thread)?;
        if let Some(tombstone) = tombstone {
            let _ = crate::store::child_store::finish_children_delete_blocking(&tombstone);
        }
        Ok(())
    }

    async fn append_event(&self, thread: ThreadId, event: RuntimeEvent) -> Result<RuntimeEvent> {
        // Resolve through the locator: a child run writes into
        // `threads/<parent>/children/<child>/`, never a phantom top-level thread.
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        let stored = writer.append(event).await?;
        self.index_set_watermark(thread, stored.seq());
        Ok(stored)
    }

    async fn replay_events(&self, thread: ThreadId, since_seq: u64) -> Result<Vec<RuntimeEvent>> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        writer.replay(since_seq).await
    }

    async fn replay_child_completion_events(&self, thread: ThreadId) -> Result<Vec<RuntimeEvent>> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        writer.child_completion_events().await
    }

    async fn highest_seq(&self, thread: ThreadId) -> Result<u64> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        writer.watermark().await
    }

    async fn read_items(&self, thread: ThreadId) -> Result<Vec<ThreadItem>> {
        let paths = self.resolve_paths(thread);
        let store = self;
        tokio::task::spawn_blocking(move || {
            if read_item_snapshots_blocking(&paths).is_empty() {
                crate::store::items_log::read_items_blocking(&paths)
            } else {
                crate::store::items_log::read_items_blocking(&paths)
            }
            .map(|items| {
                let _ = store;
                items
            })
        })
        .await
        .map_err(|err| ArkError::internal(format!("item read task failed: {err}")))?
    }

    async fn subscribe(&self, thread: ThreadId, since_seq: u64) -> Result<SubscribeTicket> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        let snapshot = writer.snapshot_and_subscribe(since_seq).await?;
        Ok(SubscribeTicket {
            backlog: snapshot.backlog,
            watermark: snapshot.watermark,
            live: snapshot.live,
        })
    }

    async fn fork_thread(&self, thread: ThreadId, upto_seq: u64) -> Result<ThreadMeta> {
        self.fork_blocking(thread, upto_seq)
    }

    async fn create_child_run(
        &self,
        parent: ThreadId,
        child: ark_common::ChildId,
        label: &str,
    ) -> Result<()> {
        self.require_thread_file(parent)?;
        // `create_child_dir` enforces the depth policy, so a child of a child is
        // refused at the one place that can actually create directories.
        self.child_store(parent).create_child_dir(child, label)?;
        Ok(())
    }

    async fn append_child_run(&self, record: ChildRunRecord) -> Result<()> {
        self.append_child_run_blocking(&record)
    }

    async fn list_child_runs(&self, parent: ThreadId) -> Result<Vec<ChildRunRecord>> {
        self.require_thread_file(parent)?;
        self.with_child_log_lock(parent, || {
            Ok(crate::store::child_store::read_child_runs_blocking(
                &self.thread_paths(parent),
            ))
        })
    }

    async fn delete_child_run(&self, parent: ThreadId, child: ark_common::ChildId) -> Result<bool> {
        self.require_thread_file(parent)?;
        self.with_child_log_lock(parent, || {
            let paths = self.thread_paths(parent);
            // Order matters: drop the record first so a crash leaves an orphan
            // directory (which reconciliation reports) rather than a record that
            // points at a directory nothing can reopen.
            let removed = crate::store::child_store::remove_child_run_blocking(
                &paths,
                child,
                self.config.limits().fsync_on_append,
            )?;
            if !removed {
                return Ok(false);
            }
            // The child's event writer must be closed before its directory goes,
            // or the actor keeps appending into an unlinked file.
            if let Some(handle) = self.registry.forget(&self.paths.child(&parent, &child)) {
                drop(handle);
            }
            crate::store::child_store::remove_child_dir_blocking(&paths, child)?;
            if let Ok(mut locator) = self.children.lock() {
                locator.remove(&child);
            }
            Ok(true)
        })
    }
}

#[async_trait::async_trait]
impl ark_ports::EventSink for FileSessionStore {
    async fn publish(&self, thread: ThreadId, event: RuntimeEvent) -> Result<RuntimeEvent> {
        self.append_event(thread, event).await
    }

    async fn subscribe(&self, thread: ThreadId, since_seq: u64) -> Result<ark_ports::Subscription> {
        let ticket = SessionStore::subscribe(self, thread, since_seq).await?;
        Ok(ark_ports::Subscription {
            backlog: ticket.backlog,
            watermark: ticket.watermark,
            live: ticket.live,
        })
    }
}

/// Compaction markers and usage increments read straight from disk.
pub(crate) fn thread_extras(
    paths: &ThreadPaths,
) -> (Vec<crate::domain::CompactionMarker>, Vec<UsageRecord>) {
    (
        read_markers_blocking(paths),
        crate::store::items_log::read_usage_blocking(paths),
    )
}

/// Fresh `ThreadSummary` for the HTTP list route.
pub(crate) fn summary_of(
    store: &FileSessionStore,
    thread: ThreadId,
    file: &ThreadFile,
) -> ThreadSummary {
    let last_seq = store
        .lock_index()
        .ok()
        .and_then(|index| index.get(&thread).map(|entry| entry.indexed_seq))
        .unwrap_or(0);
    ThreadSummary {
        thread_id: file.thread_id,
        title: file.title.clone(),
        created_at: file.created_at,
        updated_at: file.updated_at,
        last_seq: Some(last_seq),
        archived: file.archived,
    }
}

#[cfg(test)]
mod tests {
    use crate::service::store::tests::test_store;
    use crate::store::tests::{event_at, text_delta, TempDir};
    use ark_common::{ChildId, ItemId, ThreadId, TurnId};
    use ark_ports::{ChildRunRecord, SessionStore};
    use std::sync::Arc;

    fn child_record(parent: ThreadId, child: ChildId, status: &str) -> ChildRunRecord {
        let mut record =
            ChildRunRecord::queued(child, parent, TurnId::new(), "scan", chrono::Utc::now());
        record.status = status.to_string();
        if status != "queued" && status != "running" {
            record.ended_at = Some(chrono::Utc::now());
        }
        record
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_child_run_appends_are_not_lost() {
        let dir = TempDir::new("thread-ops-child-concurrent");
        let store = Arc::new(test_store(&dir));
        let meta = store.create_thread("parent").await.unwrap();

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let store = Arc::clone(&store);
            let parent = meta.thread_id;
            tasks.push(tokio::spawn(async move {
                let record = child_record(parent, ChildId::new(), "queued");
                store.append_child_run(record).await.unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        let records = store.list_child_runs(meta.thread_id).await.unwrap();
        assert_eq!(records.len(), 8, "every concurrent append must survive");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn deleting_one_child_does_not_drop_a_concurrent_append() {
        let dir = TempDir::new("thread-ops-child-delete-race");
        let store = Arc::new(test_store(&dir));
        let meta = store.create_thread("parent").await.unwrap();
        let parent = meta.thread_id;
        let victim = ChildId::new();
        let survivor = ChildId::new();
        store
            .append_child_run(child_record(parent, victim, "completed"))
            .await
            .unwrap();
        store
            .append_child_run(child_record(parent, survivor, "queued"))
            .await
            .unwrap();

        // Delete rewrites runs.jsonl (read/modify/write). A concurrent status
        // append must not be overwritten by that rewrite.
        let deleting = {
            let store = Arc::clone(&store);
            tokio::spawn(async move { store.delete_child_run(parent, victim).await.unwrap() })
        };
        let mut appends = Vec::new();
        for status in ["running", "completed"] {
            let store = Arc::clone(&store);
            appends.push(tokio::spawn(async move {
                store
                    .append_child_run(child_record(parent, survivor, status))
                    .await
                    .unwrap();
            }));
        }

        assert!(deleting.await.unwrap(), "the victim record must be found");
        for task in appends {
            task.await.unwrap();
        }

        let records = store.list_child_runs(parent).await.unwrap();
        assert!(
            records.iter().all(|record| record.child_id != victim),
            "the deleted child must not come back"
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record.child_id == survivor)
                .count(),
            3,
            "queued plus two concurrent status appends must all survive the rewrite"
        );
    }

    #[tokio::test]
    async fn appends_are_monotonic_and_durable_across_reopen() {
        let dir = TempDir::new("thread-ops-seq");
        let meta = {
            let store = test_store(&dir);
            let meta = store.create_thread("t").await.unwrap();
            for _ in 0..4 {
                store
                    .append_event(meta.thread_id, event_at(meta.thread_id, 999))
                    .await
                    .unwrap();
            }
            assert_eq!(store.highest_seq(meta.thread_id).await.unwrap(), 4);
            meta
        };

        // A restart releases the data-directory lock first; only then may a new
        // process (here: the reopened store) take it over. Holding both at once
        // is exactly the corruption case the lock refuses.
        let reopened = test_store(&dir);
        assert_eq!(reopened.highest_seq(meta.thread_id).await.unwrap(), 4);
        assert_eq!(
            reopened
                .replay_events(meta.thread_id, 2)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn items_are_projected_from_events() {
        let dir = TempDir::new("thread-ops-items");
        let store = test_store(&dir);
        let meta = store.create_thread("t").await.unwrap();
        let turn = TurnId::new();
        let item = ItemId::new();
        for (index, delta) in ["he", "llo"].iter().enumerate() {
            store
                .append_event(
                    meta.thread_id,
                    text_delta(meta.thread_id, turn, item, index as u64 + 1, delta),
                )
                .await
                .unwrap();
        }
        let items = store.read_items(meta.thread_id).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn forking_copies_the_prefix_and_retargets_thread_id() {
        let dir = TempDir::new("thread-ops-fork");
        let store = test_store(&dir);
        let meta = store.create_thread("parent").await.unwrap();
        for _ in 0..5 {
            store
                .append_event(meta.thread_id, event_at(meta.thread_id, 1))
                .await
                .unwrap();
        }
        let forked = store.fork_thread(meta.thread_id, 3).await.unwrap();
        assert_ne!(forked.thread_id, meta.thread_id);
        let events = store.replay_events(forked.thread_id, 0).await.unwrap();
        assert_eq!(events.len(), 3);
        assert!(events
            .iter()
            .all(|event| event.thread_id() == forked.thread_id));
        assert_eq!(
            store.highest_seq(forked.thread_id).await.unwrap(),
            3,
            "fork keeps the original seq numbering"
        );
    }

    #[tokio::test]
    async fn deleting_a_parent_with_an_active_child_is_a_conflict() {
        let dir = TempDir::new("thread-ops-conflict");
        let store = test_store(&dir);
        let meta = store.create_thread("parent").await.unwrap();
        let mut record = ark_ports::ChildRunRecord::queued(
            ark_common::ChildId::new(),
            meta.thread_id,
            TurnId::new(),
            "scan",
            chrono::Utc::now(),
        );
        record.status = "running".into();
        store.append_child_run(record).await.unwrap();
        let err = store.delete_thread(meta.thread_id).await.unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::Conflict);
        assert!(err.detail.unwrap().contains("running"));
    }

    #[tokio::test]
    async fn completed_children_do_not_block_deletion() {
        let dir = TempDir::new("thread-ops-delete-ok");
        let store = test_store(&dir);
        let meta = store.create_thread("parent").await.unwrap();
        let mut record = ark_ports::ChildRunRecord::queued(
            ark_common::ChildId::new(),
            meta.thread_id,
            TurnId::new(),
            "scan",
            chrono::Utc::now(),
        );
        record.status = "completed".into();
        record.ended_at = Some(chrono::Utc::now());
        store.append_child_run(record).await.unwrap();
        store.delete_thread(meta.thread_id).await.unwrap();
        assert!(!store.thread_paths(meta.thread_id).dir().exists());
    }
}
