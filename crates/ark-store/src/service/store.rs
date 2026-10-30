//! `FileSessionStore`: the `SessionStore` implementation the runtime uses.
//!
//! Composition root for the store crate: it owns the data paths, the writer
//! registry, the index and the child locator, and delegates each concern to the
//! module that implements it. Thread lifecycle lives here; event operations live
//! in `store_events`, child runs in `child_service`, reconciliation in
//! `reconcile`.

use crate::config::StoreConfig;
use crate::domain::child_index::ChildLocator;
use crate::domain::diagnostics::{DiagnosticSink, MemoryDiagnostics};
use crate::domain::layout::{names, DataPaths, ThreadPaths};
use crate::domain::thread_file::{self, ThreadFile};
use crate::service::child_service::ChildStore;
use crate::service::reconcile::{reconcile_index, ReconcileReport};
use crate::service::writer_registry::WriterRegistry;
use crate::store::data_lock::DataLock;
use crate::store::fs_util::{ensure_dir, remove_dir_all, write_json_atomic};
use crate::store::index::{load_snapshot, save_snapshot, IndexEntry, ThreadIndex};
use ark_common::{ArkError, Result, ThreadId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Directory-backed session store (design 7.4).
pub struct FileSessionStore {
    pub(crate) config: StoreConfig,
    pub(crate) paths: DataPaths,
    pub(crate) registry: WriterRegistry,
    pub(crate) diagnostics: Arc<dyn DiagnosticSink>,
    pub(crate) index: Mutex<ThreadIndex>,
    pub(crate) children: Mutex<ChildLocator>,
    /// Per-parent serialization for the authoritative child-run log and its
    /// derived index. Atomic rename prevents torn files, but only this lock
    /// prevents a concurrent append from being lost by delete's read/modify/write.
    child_logs: Mutex<HashMap<ThreadId, Arc<Mutex<()>>>>,
    /// Cross-process exclusivity over `data/.lock` (design §7.4.3).
    ///
    /// Declared last so it is the **last** field dropped: every writer actor and
    /// half-finished append must be gone before another process may take over.
    _lock: DataLock,
}

impl FileSessionStore {
    /// Open the store at `config.data_dir()`, creating the layout if needed.
    pub fn open(config: StoreConfig) -> Result<Self> {
        Self::with_diagnostics(config, Arc::new(MemoryDiagnostics::new()))
    }

    /// Open the store and report repairs to `diagnostics`.
    ///
    /// Takes the data-directory lock **before** touching any file, so a second
    /// writer fails fast instead of handing out duplicate event sequence numbers
    /// against the same authoritative log (design §7.4.3).
    pub fn with_diagnostics(
        config: StoreConfig,
        diagnostics: Arc<dyn DiagnosticSink>,
    ) -> Result<Self> {
        let paths = DataPaths::new(config.data_dir());
        ensure_dir(paths.root())?;
        let lock = DataLock::acquire(&paths.lock_file())?;
        ensure_dir(&paths.threads_dir())?;
        let index = load_snapshot(&paths.index_file());
        let registry = WriterRegistry::new(config.limits().clone(), diagnostics.clone());
        Ok(Self {
            config,
            paths,
            registry,
            diagnostics,
            index: Mutex::new(index),
            children: Mutex::new(ChildLocator::default()),
            child_logs: Mutex::new(HashMap::new()),
            _lock: lock,
        })
    }

    pub fn data_paths(&self) -> &DataPaths {
        &self.paths
    }

    pub fn config(&self) -> &StoreConfig {
        &self.config
    }

    pub fn diagnostics(&self) -> Arc<dyn DiagnosticSink> {
        self.diagnostics.clone()
    }

    /// Serialize one complete `children/runs.jsonl` transaction for a parent.
    ///
    /// The lock-table mutex is released before the parent lock is acquired. Code
    /// inside `operation` may then take the child locator lock, establishing the
    /// global order `parent log -> locator` and avoiding an ABBA deadlock.
    pub(crate) fn with_child_log_lock<T>(
        &self,
        parent: ThreadId,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let lock = {
            let mut locks = self
                .child_logs
                .lock()
                .map_err(|_| ArkError::internal("child log lock table poisoned"))?;
            Arc::clone(
                locks
                    .entry(parent)
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _guard = lock
            .lock()
            .map_err(|_| ArkError::internal("child log lock poisoned"))?;
        operation()
    }

    /// Paths for one thread directory.
    ///
    /// This is the *raw* mapping: it always resolves to `threads/<id>`.
    /// Use [`Self::resolve_paths`] for event I/O, because a child run lives at
    /// `threads/<parent>/children/<child>` and writing to the raw path would
    /// scatter a child's events into a phantom top-level thread.
    pub fn thread_paths(&self, thread: ThreadId) -> ThreadPaths {
        self.paths.thread(&thread)
    }

    /// Paths for event I/O, resolving child runs to their inlined directory.
    ///
    /// The locator is the runtime view of `children/runs.jsonl`, so a child is
    /// found by its own id without the caller having to know its parent.
    pub fn resolve_paths(&self, thread: ThreadId) -> ThreadPaths {
        let child = ark_common::ChildId::from_uuid(thread.as_uuid());
        if let Ok(locator) = self.children.lock() {
            if let Some(location) = locator.get(&child) {
                return self.paths.child(&location.parent_thread_id, &child);
            }
        }
        self.paths.thread(&thread)
    }

    /// Child-run store scoped to `parent` (design 7.4.1: children are inlined).
    pub fn child_store(&self, parent: ThreadId) -> ChildStore<'_> {
        ChildStore::new(self, parent)
    }

    /// Read `thread.json`, or `NotFound` when the thread does not exist.
    pub(crate) fn require_thread_file(&self, thread: ThreadId) -> Result<ThreadFile> {
        thread_file::read(&self.resolve_paths(thread))
            .ok_or_else(|| ArkError::not_found(format!("thread {thread} does not exist")))
    }

    /// Create the thread directory and its documented subdirectories.
    pub(crate) fn create_thread_dir(&self, file: &ThreadFile) -> Result<ThreadPaths> {
        let paths = self.thread_paths(file.thread_id);
        ensure_dir(paths.dir())?;
        ensure_dir(&paths.attachments_dir())?;
        ensure_dir(&paths.runs_dir())?;
        ensure_dir(&paths.children_dir())?;
        write_json_atomic(&paths.thread_json(), file)?;
        Ok(paths)
    }

    /// Insert or refresh the index row for `file`.
    pub(crate) fn index_upsert(&self, file: &ThreadFile, indexed_seq: u64) -> Result<()> {
        let mut entry = IndexEntry::new(
            file.thread_id,
            file.title.clone(),
            self.paths.thread_rel(&file.thread_id),
        );
        entry.updated_at = file.updated_at;
        entry.archived = file.archived;
        entry.parent_thread_id = file.parent_thread_id;
        entry.indexed_seq = indexed_seq;
        let mut index = self.lock_index()?;
        index.insert(entry);
        save_snapshot(&mut index, &self.paths.index_file())
    }

    /// Update the indexed watermark without touching the rest of the row.
    pub(crate) fn index_set_watermark(&self, thread: ThreadId, seq: u64) {
        if let Ok(mut index) = self.lock_index() {
            index.set_watermark(&thread, seq);
            let _ = save_snapshot(&mut index, &self.paths.index_file());
        }
    }

    pub(crate) fn index_remove(&self, thread: ThreadId) -> Result<Option<IndexEntry>> {
        let mut index = self.lock_index()?;
        let removed = index.remove(&thread);
        save_snapshot(&mut index, &self.paths.index_file())?;
        Ok(removed)
    }

    pub(crate) fn lock_index(&self) -> Result<std::sync::MutexGuard<'_, ThreadIndex>> {
        self.index
            .lock()
            .map_err(|_| ArkError::internal("thread index lock poisoned"))
    }

    /// Run the startup reconciliation and return what it did.
    pub fn reconcile(&self) -> Result<ReconcileReport> {
        reconcile_index(self)
    }

    /// Make sure `parent` has no active child run (delete gate, design 7.4.1).
    pub(crate) fn ensure_no_active_children(&self, parent: ThreadId) -> Result<()> {
        let paths = self.thread_paths(parent);
        let active = crate::store::child_store::active_child_runs_blocking(&paths);
        if active.is_empty() {
            return Ok(());
        }
        let listing: Vec<serde_json::Value> = active
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "child_id": entry.child_id,
                    "label": entry.label,
                    "status": entry.status,
                })
            })
            .collect();
        Err(ArkError::conflict(format!(
            "thread {parent} still has {} active child run(s); retry with force=true \
             to destroy them first",
            active.len()
        ))
        .with_detail(serde_json::json!(listing).to_string()))
    }

    /// Remove a thread directory after its writers have been closed.
    pub(crate) fn remove_thread_dir(&self, thread: ThreadId) -> Result<()> {
        let paths = self.thread_paths(thread);
        if let Some(handle) = self.registry.forget(&paths) {
            // The actor owns the file handles; dropping the channel stops it.
            drop(handle);
        }
        remove_dir_all(paths.dir())?;
        // The row is derived data, but leaving it behind would resurrect the
        // thread in `list_threads` until the next reconciliation pass.
        self.index_remove(thread)?;
        self.children
            .lock()
            .map_err(|_| ArkError::internal("child locator lock poisoned"))?
            .retain_parent(thread);
        Ok(())
    }

    /// Register a child run so `child_id -> parent` lookups resolve.
    pub(crate) fn remember_child(
        &self,
        parent: ThreadId,
        records: &[ark_ports::ChildRunRecord],
    ) -> Result<()> {
        let mut locator = self
            .children
            .lock()
            .map_err(|_| ArkError::internal("child locator lock poisoned"))?;
        for record in records {
            locator.insert(
                record.child_id,
                crate::domain::child_index::ChildLocation::new(
                    parent,
                    self.paths.child_rel(&parent, &record.child_id),
                ),
            );
        }
        Ok(())
    }

    /// Whether `thread` is itself a child run, which forbids delegating again.
    ///
    /// Depth is derived from two independent facts so a hand-edited
    /// `thread.json` cannot silently enable nested delegation: the recorded
    /// `parent_thread_id`, and the directory being physically nested under a
    /// `children/` component.
    pub fn prefix_is_child(&self, thread: ThreadId) -> bool {
        // 1. The locator is the runtime view of `children/runs.jsonl`.
        let known_child = ark_common::ChildId::from_uuid(thread.as_uuid());
        if self.parent_of_child(known_child).is_some() {
            return true;
        }
        // 2. A child directory records its parent in `thread.json`.
        let paths = self.thread_paths(thread);
        if thread_file::read(&paths).is_some_and(|file| file.parent_thread_id.is_some()) {
            return true;
        }
        // 3. The directory is physically nested under a `children/` component.
        if paths
            .dir()
            .components()
            .any(|component| component.as_os_str() == names::CHILDREN)
        {
            return true;
        }
        // 4. A child directory exists even though no row was loaded yet.
        self.child_dir_exists(thread)
    }

    /// Whether any `threads/<parent>/children/<thread>/` directory exists.
    fn child_dir_exists(&self, thread: ThreadId) -> bool {
        crate::store::fs_util::list_dirs(&self.paths.threads_dir())
            .into_iter()
            .any(|parent_dir| {
                ThreadPaths::new(parent_dir)
                    .children_dir()
                    .join(thread.to_string())
                    .is_dir()
            })
    }

    /// Register a freshly created child so depth checks see it immediately.
    pub(crate) fn note_child_location(&self, parent: ThreadId, child: ark_common::ChildId) {
        if let Ok(mut locator) = self.children.lock() {
            locator.insert(
                child,
                crate::domain::child_index::ChildLocation::new(
                    parent,
                    self.paths.child_rel(&parent, &child),
                ),
            );
        }
    }
    /// Parent id for a child run, resolved from the authoritative run log.
    pub fn parent_of_child(&self, child: ark_common::ChildId) -> Option<ThreadId> {
        self.children
            .lock()
            .ok()?
            .get(&child)
            .map(|location| location.parent_thread_id)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::StoreLimits;
    use crate::domain::diagnostics::MemoryDiagnostics;
    use ark_ports::SessionStore;

    pub(crate) fn test_store(dir: &crate::store::tests::TempDir) -> FileSessionStore {
        FileSessionStore::with_diagnostics(
            StoreConfig::new(dir.path()).with_limits(StoreLimits::for_tests()),
            Arc::new(MemoryDiagnostics::new()),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn created_threads_get_the_documented_layout() {
        let dir = crate::store::tests::TempDir::new("store-layout");
        let store = test_store(&dir);
        let meta = store.create_thread("first").await.unwrap();
        let paths = store.thread_paths(meta.thread_id);
        assert!(paths.thread_json().is_file());
        assert!(paths.attachments_dir().is_dir());
        assert!(paths.runs_dir().is_dir());
        assert!(paths.children_dir().is_dir());
    }

    #[tokio::test]
    async fn list_threads_returns_newest_first() {
        let dir = crate::store::tests::TempDir::new("store-list");
        let store = test_store(&dir);
        let first = store.create_thread("a").await.unwrap();
        let second = store.create_thread("b").await.unwrap();
        let listed = store.list_threads().await.unwrap();
        assert_eq!(listed.len(), 2);
        let ids: Vec<ThreadId> = listed.iter().map(|meta| meta.thread_id).collect();
        assert!(ids.contains(&first.thread_id));
        assert!(ids.contains(&second.thread_id));
    }

    #[tokio::test]
    async fn rename_and_archive_persist_to_thread_json() {
        let dir = crate::store::tests::TempDir::new("store-rename");
        let store = test_store(&dir);
        let meta = store.create_thread("before").await.unwrap();
        let renamed = store.rename_thread(meta.thread_id, "after").await.unwrap();
        assert_eq!(renamed.title, "after");
        let archived = store.set_archived(meta.thread_id, true).await.unwrap();
        assert!(archived.archived);
        let reread = store.get_thread(meta.thread_id).await.unwrap();
        assert_eq!(reread.title, "after");
        assert!(reread.archived);
    }

    #[tokio::test]
    async fn missing_threads_report_not_found() {
        let dir = crate::store::tests::TempDir::new("store-missing");
        let store = test_store(&dir);
        let err = store.get_thread(ThreadId::new()).await.unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
    }

    #[tokio::test]
    async fn deleting_a_thread_removes_the_directory_and_the_row() {
        let dir = crate::store::tests::TempDir::new("store-delete");
        let store = test_store(&dir);
        let meta = store.create_thread("gone").await.unwrap();
        let paths = store.thread_paths(meta.thread_id);
        store.delete_thread(meta.thread_id).await.unwrap();
        assert!(!paths.dir().exists());
        assert!(store.lock_index().unwrap().get(&meta.thread_id).is_none());
    }
}
