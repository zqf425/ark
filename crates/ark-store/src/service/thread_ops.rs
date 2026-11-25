//! Thread lifecycle and the `SessionStore` / `EventSink` implementations.
//!
//! Every event operation goes through the per-thread writer actor, so `seq`
//! allocation, the durable append and the subscribe watermark all share one
//! serialization point (design 7.4.3).

use crate::domain::thread_file::ThreadFile;
use crate::service::store::FileSessionStore;
use crate::store::child_store::rebuild_child_index_blocking;
use crate::store::index::IndexEntry;
use ark_common::{ArkError, Result, ThreadId};
use ark_contract::{RuntimeEvent, ThreadItem};
use ark_ports::{ChildRunRecord, SessionStore, SubscribeTicket, ThreadMeta};

impl FileSessionStore {
    /// Attach the derived event watermark to a metadata row.
    ///
    /// `ThreadFile::to_meta` is a pure domain mapping and cannot see the index;
    /// the store fills in `last_seq` so every `SessionStore` result (list, get,
    /// rename, archive, runtime, fork) carries the same durable watermark. The
    /// server maps that straight onto `ThreadSummary`, which is what the SSE
    /// client uses as `since_seq`.
    fn meta_with_seq(&self, file: &ThreadFile) -> Result<ThreadMeta> {
        let mut meta = file.to_meta();
        meta.last_seq = Some(self.highest_seq_of(file.thread_id)?);
        Ok(meta)
    }

    /// Create the thread directory, the metadata file and the index row.
    pub(crate) fn create_thread_blocking(&self, title: &str) -> Result<ThreadMeta> {
        let file = ThreadFile::new(ThreadId::new(), title);
        self.create_thread_dir(&file)?;
        self.index_upsert(&file, 0)?;
        self.meta_with_seq(&file)
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
        crate::store::fs_util::write_json_atomic(&self.resolve_paths(thread).thread_json(), &file)?;
        if file.parent_thread_id.is_none() {
            self.index_upsert(&file, self.highest_seq_of(thread)?)?;
        }
        self.meta_with_seq(&file)
    }

    pub(crate) fn set_archived_blocking(
        &self,
        thread: ThreadId,
        archived: bool,
    ) -> Result<ThreadMeta> {
        let mut file = self.require_thread_file(thread)?;
        file.archived = archived;
        file.touch();
        crate::store::fs_util::write_json_atomic(&self.resolve_paths(thread).thread_json(), &file)?;
        if file.parent_thread_id.is_none() {
            self.index_upsert(&file, self.highest_seq_of(thread)?)?;
        }
        self.meta_with_seq(&file)
    }

    pub(crate) fn set_thread_runtime_blocking(
        &self,
        thread: ThreadId,
        runtime: ark_contract::ThreadRuntimeSettings,
    ) -> Result<ThreadMeta> {
        let mut file = self.require_thread_file(thread)?;
        file.workspace = runtime.workspace;
        file.provider_id = runtime.provider_id;
        file.model = runtime.model;
        file.autonomy = runtime.autonomy;
        file.reasoning_effort = runtime.reasoning_effort;
        file.touch();
        crate::store::fs_util::write_json_atomic(&self.resolve_paths(thread).thread_json(), &file)?;
        if file.parent_thread_id.is_none() {
            self.index_upsert(&file, self.highest_seq_of(thread)?)?;
        }
        self.meta_with_seq(&file)
    }

    /// Highest durable `seq` for a metadata refresh: the index watermark first,
    /// the log read only as fallback.
    ///
    /// The index watermark only ever lags: appends refresh it right after the
    /// durable append, rewind resets it to `last_seq`, and startup reconcile
    /// catches a stale row up. Reading the whole log here would make every
    /// rename/archive/settings edit pay an O(transcript) scan for a number the
    /// index already carries. A missing row (deleted index snapshot, fresh
    /// process before reconcile) falls back to the log so the refresh still
    /// writes the real watermark instead of regressing the index to 0.
    pub(crate) fn highest_seq_of(&self, thread: ThreadId) -> Result<u64> {
        if let Ok(index) = self.lock_index() {
            if let Some(entry) = index.get(&thread) {
                return Ok(entry.indexed_seq);
            }
        }
        // Propagate the log error: falling back to 0 on a corrupt log would
        // silently rewrite the index watermark while the bad line stays on disk.
        Ok(crate::store::event_log::read_log_blocking(&self.resolve_paths(thread))?.max_seq)
    }

    /// Write a forked copy of `(0, upto_seq]` into a brand-new thread directory.
    pub(crate) fn fork_blocking(&self, thread: ThreadId, upto_seq: u64) -> Result<ThreadMeta> {
        let source = self.require_thread_file(thread)?;
        let source_paths = self.resolve_paths(thread);
        if crate::store::fs_util::path_entry_exists(&source_paths.compacted_jsonl()) {
            return Err(ArkError::conflict(format!(
                "cannot fork thread {thread}: source log is compacted"
            )));
        }
        let events = crate::store::event_log::read_log_blocking(&source_paths)?
            .events
            .into_iter()
            .filter(|event| event.seq() <= upto_seq)
            .collect::<Vec<_>>();
        let source_max = events.iter().map(RuntimeEvent::seq).max().unwrap_or(0);
        let copied_max = events.last().map(RuntimeEvent::seq).unwrap_or(0);
        if upto_seq > source_max || (upto_seq > 0 && copied_max != upto_seq) {
            return Err(ArkError::validation(format!(
                "cannot fork thread {thread} at seq {upto_seq}: source log ends at {source_max}"
            )));
        }
        let target = ThreadId::new();
        let mut forked = source.forked_from(target);
        forked.fork_at_seq = Some(upto_seq);
        let paths = self.create_thread_dir(&forked)?;
        let mut body = String::new();
        for event in &events {
            let retargeted = crate::domain::retarget::retarget_event(event, target);
            body.push_str(&crate::store::event_log::encode_event(&retargeted)?);
        }
        crate::store::fs_util::write_atomic(&paths.events_jsonl(), body.as_bytes())?;
        // The copied events project to a render prefix. Record its size so the
        // canvas can fold exactly these inherited rows instead of presenting the
        // fork as a second copy of the whole source transcript.
        let inherited = crate::store::items_log::rebuild_items_blocking(&paths)?;
        forked.inherited_item_count = Some(inherited.len() as u32);
        crate::store::fs_util::write_json_atomic(&paths.thread_json(), &forked)?;
        self.index_upsert(&forked, upto_seq)?;
        let mut meta = forked.to_meta();
        meta.last_seq = Some(upto_seq);
        Ok(meta)
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
            let parent = record.parent_thread_id;
            let parent_paths = self.thread_paths(parent);
            let child_paths = self.paths.child(&parent, &record.child_id);
            self.registry.ensure_not_deleting(&child_paths)?;
            let location = crate::domain::child_index::ChildLocation::new(
                parent,
                self.paths.child_rel(&parent, &record.child_id),
            );
            let mut locator = self
                .children
                .lock()
                .map_err(|_| ArkError::internal("child locator lock poisoned"))?;
            locator.ensure_available(record.child_id, &location)?;
            crate::store::child_store::append_child_run_blocking(
                &parent_paths,
                record,
                self.config.limits().fsync_on_append,
            )?;
            locator.insert(record.child_id, location);
            // Cache only: a failure here is reported, never propagated, because
            // the next read or reconcile rebuilds it from the log.
            if let Err(err) = rebuild_child_index_blocking(&parent_paths) {
                tracing::warn!(
                    parent = %parent,
                    child = %record.child_id,
                    error = %err.message,
                    "child-run record stored but its derived index could not be rebuilt"
                );
            }
            Ok(())
        })
    }
}

fn entry_to_meta(entry: &IndexEntry) -> ThreadMeta {
    ThreadMeta {
        thread_id: entry.thread_id,
        title: entry.title.clone(),
        // The index carries the real creation time from `thread.json`; older
        // snapshots without the field fall back to the last update time.
        created_at: entry.created_at.unwrap_or(entry.updated_at),
        updated_at: entry.updated_at,
        archived: entry.archived,
        // The mirror keeps the list's project grouping in step with
        // `thread.json` without reopening every metadata file.
        runtime: ark_contract::ThreadRuntimeSettings {
            workspace: entry.workspace.clone(),
            ..Default::default()
        },
        forked_from: entry.forked_from,
        fork_at_seq: entry.fork_at_seq,
        inherited_item_count: entry.inherited_item_count,
        // The index watermark is the durable event high-water mark. Exposing it
        // here keeps the list watermark in step with the detail watermark, so an
        // SSE subscribe never replays history the canvas already seeded.
        last_seq: Some(entry.indexed_seq),
        last_event_at: entry.last_event_at,
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
        self.meta_with_seq(&self.require_thread_file(thread)?)
    }

    async fn rename_thread(&self, thread: ThreadId, title: &str) -> Result<ThreadMeta> {
        self.rename_thread_blocking(thread, title)
    }

    async fn set_archived(&self, thread: ThreadId, archived: bool) -> Result<ThreadMeta> {
        self.set_archived_blocking(thread, archived)
    }

    async fn set_thread_runtime(
        &self,
        thread: ThreadId,
        runtime: ark_contract::ThreadRuntimeSettings,
    ) -> Result<ThreadMeta> {
        self.set_thread_runtime_blocking(thread, runtime)
    }

    async fn delete_thread(&self, thread: ThreadId) -> Result<()> {
        self.require_thread_file(thread)?;
        self.ensure_no_active_children(thread)?;
        let paths = self.thread_paths(thread);
        let deletion = self.registry.begin_delete(&paths)?;
        deletion.drain().await;
        let tombstone = self.child_store(thread).tombstone_children();
        let result = (|| {
            self.remove_thread_dir(thread)?;
            if let Some(tombstone) = tombstone {
                let _ = crate::store::child_store::finish_children_delete_blocking(&tombstone);
            }
            Ok(())
        })();
        deletion.complete(result.is_ok())?;
        result
    }

    async fn append_event(&self, thread: ThreadId, event: RuntimeEvent) -> Result<RuntimeEvent> {
        // Resolve through the locator: a child run writes into
        // `threads/<parent>/children/<child>/`, never a phantom top-level thread.
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        let stored = writer.append(event).await?;
        // Test-only scheduling point: the actor has already updated the index.
        // Delaying this caller's completion must not change the watermark.
        #[cfg(test)]
        self.wait_for_index_test_gate().await;
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

    async fn replay_interaction_events(&self, thread: ThreadId) -> Result<Vec<RuntimeEvent>> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        writer.interaction_events().await
    }

    async fn replay_attachment_events(&self, thread: ThreadId) -> Result<Vec<RuntimeEvent>> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        writer.attachment_events().await
    }

    async fn highest_seq(&self, thread: ThreadId) -> Result<u64> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        writer.watermark().await
    }

    async fn read_items(&self, thread: ThreadId) -> Result<Vec<ThreadItem>> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        // The actor first completes any pending rewind, then reads the disk
        // projection while serialized with every write for this thread.
        writer.read_items().await
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

    async fn rewind_thread(
        &self,
        thread: ThreadId,
        rewind_from_seq: u64,
        target_turn_id: Option<ark_common::TurnId>,
    ) -> Result<ark_contract::RewindResponse> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        let result = writer.rewind(rewind_from_seq, target_turn_id).await?;
        // The actor recomputed both watermark and last activity before acking.
        #[cfg(test)]
        self.wait_for_index_test_gate().await;
        Ok(result)
    }

    async fn rewind_record(
        &self,
        thread: ThreadId,
        rewind_from_seq: Option<u64>,
        target_turn_id: Option<ark_common::TurnId>,
    ) -> Result<Option<ark_contract::RewindResponse>> {
        let paths = self.resolve_paths(thread);
        let writer = self.registry.writer_for(thread, &paths)?;
        writer.rewind_record(rewind_from_seq, target_turn_id).await
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
        self.with_child_log_lock(parent, || {
            let paths = self.paths.child(&parent, &child);
            self.registry.ensure_not_deleting(&paths)?;
            self.child_store(parent)
                .create_child_dir(child, label)
                .map(|_| ())
        })
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
        let paths = self.thread_paths(parent);
        let child_paths = self.paths.child(&parent, &child);
        // Block new event writers first, then drain without holding the parent
        // log lock. The authoritative record is removed only inside the final
        // synchronous critical section, so cancellation leaves it replayable.
        let deletion = self.registry.begin_delete(&child_paths)?;
        deletion.drain().await;
        #[cfg(test)]
        self.wait_for_child_delete_test_gate().await;
        self.with_child_log_lock(parent, || {
            let removed = crate::store::child_store::remove_child_run_blocking(
                &paths,
                child,
                self.config.limits().fsync_on_append,
            )?;
            if !removed {
                deletion.complete(false)?;
                return Ok(false);
            }
            let result = crate::store::child_store::remove_child_dir_blocking(&paths, child);
            if result.is_ok() {
                if let Ok(mut locator) = self.children.lock() {
                    locator.remove(&child);
                }
            }
            deletion.complete(result.is_ok())?;
            result?;
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

#[cfg(test)]
mod tests {
    use crate::service::store::tests::test_store;
    use crate::store::tests::{event_at, text_delta, TempDir};
    use ark_common::{ChildId, ItemId, ThreadId, TurnId};
    use ark_ports::{ChildRunRecord, SessionStore};
    use std::sync::Arc;

    fn child_record(
        parent: ThreadId,
        child: ChildId,
        status: ark_common::ChildRunStatus,
    ) -> ChildRunRecord {
        let mut record =
            ChildRunRecord::queued(child, parent, TurnId::new(), "scan", chrono::Utc::now());
        record.status = status;
        if !matches!(
            status,
            ark_common::ChildRunStatus::Queued | ark_common::ChildRunStatus::Running
        ) {
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
                let record =
                    child_record(parent, ChildId::new(), ark_common::ChildRunStatus::Queued);
                store.append_child_run(record).await.unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        let records = store.list_child_runs(meta.thread_id).await.unwrap();
        assert_eq!(records.len(), 8, "every concurrent append must survive");
    }

    #[tokio::test]
    async fn child_id_cannot_be_appended_under_two_parents() {
        let dir = TempDir::new("thread-ops-child-global-owner");
        let store = test_store(&dir);
        let first = store.create_thread("first").await.unwrap().thread_id;
        let second = store.create_thread("second").await.unwrap().thread_id;
        let child = ChildId::new();

        store
            .append_child_run(child_record(
                first,
                child,
                ark_common::ChildRunStatus::Queued,
            ))
            .await
            .unwrap();
        store
            .append_child_run(child_record(
                first,
                child,
                ark_common::ChildRunStatus::Running,
            ))
            .await
            .unwrap();

        let error = store
            .append_child_run(child_record(
                second,
                child,
                ark_common::ChildRunStatus::Queued,
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code, ark_common::ArkErrorCode::Conflict);
        assert_eq!(store.list_child_runs(first).await.unwrap().len(), 2);
        assert!(store.list_child_runs(second).await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_cross_parent_child_appends_have_one_winner() {
        let dir = TempDir::new("thread-ops-child-global-owner-race");
        let store = Arc::new(test_store(&dir));
        let first = store.create_thread("first").await.unwrap().thread_id;
        let second = store.create_thread("second").await.unwrap().thread_id;
        let child = ChildId::new();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let mut tasks = Vec::new();
        for parent in [first, second] {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                store
                    .append_child_run(child_record(
                        parent,
                        child,
                        ark_common::ChildRunStatus::Queued,
                    ))
                    .await
            }));
        }
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap());
        }

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let loser = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("one parent must lose ownership");
        assert_eq!(loser.code, ark_common::ArkErrorCode::Conflict);
        let stored = store.list_child_runs(first).await.unwrap().len()
            + store.list_child_runs(second).await.unwrap().len();
        assert_eq!(stored, 1, "the losing parent must not append a log row");
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
            .append_child_run(child_record(
                parent,
                victim,
                ark_common::ChildRunStatus::Completed,
            ))
            .await
            .unwrap();
        store
            .append_child_run(child_record(
                parent,
                survivor,
                ark_common::ChildRunStatus::Queued,
            ))
            .await
            .unwrap();

        // Delete rewrites runs.jsonl (read/modify/write). A concurrent status
        // append must not be overwritten by that rewrite.
        let deleting = {
            let store = Arc::clone(&store);
            tokio::spawn(async move { store.delete_child_run(parent, victim).await.unwrap() })
        };
        let mut appends = Vec::new();
        for status in [
            ark_common::ChildRunStatus::Running,
            ark_common::ChildRunStatus::Completed,
        ] {
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
    async fn fork_rejects_a_watermark_beyond_the_source_log() {
        let dir = TempDir::new("thread-ops-fork-watermark");
        let store = test_store(&dir);
        let meta = store.create_thread("parent").await.unwrap();
        for _ in 0..3 {
            store
                .append_event(meta.thread_id, event_at(meta.thread_id, 1))
                .await
                .unwrap();
        }

        let error = store.fork_thread(meta.thread_id, 5).await.unwrap_err();
        assert_eq!(error.code, ark_common::ArkErrorCode::ValidationError);
        assert_eq!(store.list_threads().await.unwrap().len(), 1);
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
        record.status = ark_common::ChildRunStatus::Running;
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
        record.status = ark_common::ChildRunStatus::Completed;
        record.ended_at = Some(chrono::Utc::now());
        store.append_child_run(record).await.unwrap();
        store.delete_thread(meta.thread_id).await.unwrap();
        assert!(!store.thread_paths(meta.thread_id).dir().exists());
    }

    #[tokio::test]
    async fn rename_carries_the_index_watermark_after_appends() {
        let dir = TempDir::new("thread-ops-index-watermark");
        let store = test_store(&dir);
        let meta = store.create_thread("t").await.unwrap();
        for _ in 0..3 {
            store
                .append_event(meta.thread_id, event_at(meta.thread_id, 1))
                .await
                .unwrap();
        }

        store
            .rename_thread(meta.thread_id, "renamed")
            .await
            .unwrap();

        let entry = store
            .lock_index()
            .unwrap()
            .get(&meta.thread_id)
            .cloned()
            .unwrap();
        assert_eq!(
            entry.indexed_seq, 3,
            "rename must persist the index watermark, not reset it"
        );
    }

    #[tokio::test]
    async fn rename_falls_back_to_the_log_when_the_index_row_is_missing() {
        let dir = TempDir::new("thread-ops-index-missing");
        let store = test_store(&dir);
        let meta = store.create_thread("t").await.unwrap();
        store
            .append_event(meta.thread_id, event_at(meta.thread_id, 1))
            .await
            .unwrap();
        assert!(store.index_remove(meta.thread_id).unwrap().is_some());

        store
            .rename_thread(meta.thread_id, "renamed")
            .await
            .unwrap();

        let entry = store
            .lock_index()
            .unwrap()
            .get(&meta.thread_id)
            .cloned()
            .unwrap();
        assert_eq!(
            entry.indexed_seq, 1,
            "without an index row the watermark must come from the log"
        );
    }

    #[tokio::test]
    async fn rename_keeps_the_watermark_after_a_store_reopen() {
        let dir = TempDir::new("thread-ops-index-reopen");
        let meta = {
            let store = test_store(&dir);
            let meta = store.create_thread("t").await.unwrap();
            for _ in 0..2 {
                store
                    .append_event(meta.thread_id, event_at(meta.thread_id, 1))
                    .await
                    .unwrap();
            }
            meta
        };
        let reopened = test_store(&dir);
        // Deleting the log makes the discriminator sharp: a fallback to the log
        // would see 0, so a correct watermark here can only come from the
        // persisted index snapshot.
        std::fs::remove_file(reopened.thread_paths(meta.thread_id).events_jsonl()).unwrap();

        reopened
            .rename_thread(meta.thread_id, "renamed")
            .await
            .unwrap();

        let entry = reopened
            .lock_index()
            .unwrap()
            .get(&meta.thread_id)
            .cloned()
            .unwrap();
        assert_eq!(entry.indexed_seq, 2, "the watermark must survive a reopen");
    }

    #[tokio::test]
    async fn list_threads_keeps_created_at_consistent_with_details_after_edits() {
        let dir = TempDir::new("thread-ops-created-at");
        let store = test_store(&dir);
        let created = store.create_thread("original").await.unwrap();

        // Make `updated_at` observably newer: without this the old
        // `created_at: entry.updated_at` bug would be invisible in the assert.
        std::thread::sleep(std::time::Duration::from_millis(10));
        store
            .rename_thread(created.thread_id, "renamed")
            .await
            .unwrap();
        store.set_archived(created.thread_id, true).await.unwrap();

        let listed = store.list_threads().await.unwrap();
        let meta = listed
            .iter()
            .find(|meta| meta.thread_id == created.thread_id)
            .expect("the thread is listed");
        let detail = store.get_thread(created.thread_id).await.unwrap();

        assert!(
            meta.updated_at > meta.created_at,
            "the edit must have moved updated_at for this assertion to be sharp"
        );
        assert_eq!(meta.created_at, created.created_at);
        assert_eq!(meta.created_at, detail.created_at);
        assert_eq!(meta.updated_at, detail.updated_at);

        // The value must also survive a reopen from the persisted index snapshot.
        let id = created.thread_id;
        let expected = created.created_at;
        drop(store);
        let reopened = test_store(&dir);
        let relisted = reopened.list_threads().await.unwrap();
        let restored = relisted
            .iter()
            .find(|meta| meta.thread_id == id)
            .expect("the thread is listed after reopen");
        assert_eq!(restored.created_at, expected);
        assert_eq!(
            restored.created_at,
            reopened.get_thread(id).await.unwrap().created_at
        );
    }

    #[test]
    fn legacy_index_json_without_created_at_falls_back_to_updated_at() {
        use crate::store::index::{IndexEntry, IndexSnapshot};

        let row = IndexEntry::new(
            ThreadId::new(),
            "legacy",
            "threads/legacy",
            chrono::Utc::now(),
        );
        let mut snapshot = serde_json::to_value(IndexSnapshot::from_entries(vec![row])).unwrap();
        // Simulate an `index.json` written before `created_at` existed.
        snapshot["threads"][0]
            .as_object_mut()
            .unwrap()
            .remove("created_at");
        let legacy_text = serde_json::to_string(&snapshot).unwrap();
        assert!(!legacy_text.contains("created_at"));

        let decoded: IndexSnapshot = serde_json::from_str(&legacy_text).unwrap();
        assert_eq!(decoded.threads.len(), 1);
        let row = &decoded.threads[0];
        assert_eq!(row.created_at, None);

        let meta = super::entry_to_meta(row);
        assert_eq!(meta.created_at, row.updated_at);
        assert_eq!(meta.updated_at, row.updated_at);
    }
}

#[cfg(test)]
#[path = "thread_ops_review_tests.rs"]
mod review_tests;
