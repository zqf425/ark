//! Per-thread writer registry: one live actor per thread directory.
//!
//! Design 7.4.3 requires exactly one writer per session directory. The registry
//! is the mapping that enforces it: `writer_for` returns the existing handle if
//! one is alive, and only then opens a new `WriterState`.

use crate::config::StoreLimits;
use crate::domain::diagnostics::DiagnosticSink;
use crate::domain::layout::ThreadPaths;
use crate::store::writer_actor::{WriterHandle, WriterObserver};
use crate::store::writer_state::WriterState;
use ark_common::{ArkError, Result, ThreadId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Capacity of the per-writer command queue.
const WRITER_QUEUE_CAPACITY: usize = 256;

#[derive(Default)]
struct RegistryState {
    writers: HashMap<String, WriterHandle>,
    deleting: Vec<PathBuf>,
    deleted: Vec<PathBuf>,
}

/// Map from directory key to live writer handle.
pub struct WriterRegistry {
    limits: StoreLimits,
    diagnostics: Arc<dyn DiagnosticSink>,
    observer: Arc<dyn WriterObserver>,
    inner: Mutex<RegistryState>,
}

/// Blocks new writers below one directory, then drains every existing actor.
///
/// Dropping an unfinished guard reopens acquisition, which makes cancellation
/// before filesystem deletion fail safe rather than permanently blocking a live
/// thread. Successful deletion leaves a tombstone root until the store closes.
pub(crate) struct WriterDeleteGuard<'a> {
    registry: &'a WriterRegistry,
    root: PathBuf,
    handles: Vec<WriterHandle>,
    completed: bool,
}

impl WriterDeleteGuard<'_> {
    pub(crate) async fn drain(&self) {
        for handle in &self.handles {
            handle.shutdown().await;
        }
    }

    pub(crate) fn complete(mut self, removed: bool) -> Result<()> {
        self.registry.finish_delete(&self.root, removed)?;
        self.completed = true;
        Ok(())
    }
}

impl Drop for WriterDeleteGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.registry.finish_delete(&self.root, false);
        }
    }
}

impl WriterRegistry {
    pub fn new(
        limits: StoreLimits,
        diagnostics: Arc<dyn DiagnosticSink>,
        observer: Arc<dyn WriterObserver>,
    ) -> Self {
        Self {
            limits,
            diagnostics,
            observer,
            inner: Mutex::new(RegistryState::default()),
        }
    }

    /// Get the live writer for `paths`, opening it when necessary.
    pub fn writer_for(&self, thread: ThreadId, paths: &ThreadPaths) -> Result<WriterHandle> {
        let key = paths.key();
        let mut guard = self.lock()?;
        if guard
            .deleting
            .iter()
            .any(|root| paths.dir().starts_with(root))
        {
            return Err(ArkError::conflict(format!(
                "thread {thread} is being deleted"
            )));
        }
        if guard
            .deleted
            .iter()
            .any(|root| paths.dir().starts_with(root))
        {
            return Err(ArkError::not_found(format!(
                "thread {thread} does not exist"
            )));
        }
        // A late caller must never recreate a directory removed by an earlier
        // delete, including after a store restart when no in-memory tombstone
        // has been loaded yet.
        if !paths.exists() {
            return Err(ArkError::not_found(format!(
                "thread {thread} does not exist"
            )));
        }
        if let Some(handle) = guard.writers.get(&key) {
            if handle.is_alive() {
                return Ok(handle.clone());
            }
            guard.writers.remove(&key);
        }
        let state = WriterState::open(
            thread,
            paths.clone(),
            self.limits.clone(),
            self.diagnostics.clone(),
        )?;
        let handle =
            WriterHandle::spawn_observed(state, WRITER_QUEUE_CAPACITY, self.observer.clone());
        guard.writers.insert(key, handle.clone());
        Ok(handle)
    }

    /// Reject child-log mutations below a directory being deleted or already removed.
    pub(crate) fn ensure_not_deleting(&self, paths: &ThreadPaths) -> Result<()> {
        let guard = self.lock()?;
        if guard
            .deleting
            .iter()
            .any(|root| paths.dir().starts_with(root))
        {
            return Err(ArkError::conflict("child session is being deleted"));
        }
        if guard
            .deleted
            .iter()
            .any(|root| paths.dir().starts_with(root))
        {
            return Err(ArkError::not_found("child session does not exist"));
        }
        Ok(())
    }

    /// Atomically block acquisition below `paths` and detach its live writers.
    pub(crate) fn begin_delete(&self, paths: &ThreadPaths) -> Result<WriterDeleteGuard<'_>> {
        let root = paths.dir().to_path_buf();
        let mut guard = self.lock()?;
        if guard
            .deleting
            .iter()
            .any(|existing| paths_overlap(existing, &root))
        {
            return Err(ArkError::conflict("thread deletion is already in progress"));
        }
        if guard
            .deleted
            .iter()
            .any(|existing| paths_overlap(existing, &root))
        {
            return Err(ArkError::not_found("thread has already been deleted"));
        }
        guard.deleting.push(root.clone());
        let mut handles = Vec::new();
        guard.writers.retain(|key, handle| {
            if Path::new(key).starts_with(&root) {
                handles.push(handle.clone());
                false
            } else {
                true
            }
        });
        Ok(WriterDeleteGuard {
            registry: self,
            root,
            handles,
            completed: false,
        })
    }

    fn finish_delete(&self, root: &Path, removed: bool) -> Result<()> {
        let mut guard = self.lock()?;
        guard.deleting.retain(|path| path != root);
        if removed && !guard.deleted.iter().any(|path| path == root) {
            guard.deleted.push(root.to_path_buf());
        }
        Ok(())
    }

    /// Drop the handle for `paths` without touching the directory.
    pub fn forget(&self, paths: &ThreadPaths) -> Option<WriterHandle> {
        self.lock().ok()?.writers.remove(&paths.key())
    }

    /// All live handles, used to close writers before a directory is removed.
    pub fn drain(&self) -> Vec<WriterHandle> {
        match self.lock() {
            Ok(mut guard) => guard.writers.drain().map(|(_, handle)| handle).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn live_count(&self) -> usize {
        self.lock().map(|guard| guard.writers.len()).unwrap_or(0)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RegistryState>> {
        self.inner
            .lock()
            .map_err(|_| ArkError::internal("writer registry lock poisoned"))
    }
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::diagnostics::MemoryDiagnostics;
    use crate::store::tests::TempDir;
    use crate::store::writer_actor::NoopWriterObserver;

    fn registry() -> WriterRegistry {
        WriterRegistry::new(
            StoreLimits::for_tests(),
            Arc::new(MemoryDiagnostics::new()),
            Arc::new(NoopWriterObserver),
        )
    }

    fn thread_paths(dir: &TempDir) -> (ThreadPaths, ThreadId) {
        let thread = ThreadId::new();
        let paths = ThreadPaths::new(dir.path().join(thread.to_string()));
        crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
        crate::store::fs_util::write_json_atomic(
            &paths.thread_json(),
            &crate::domain::thread_file::ThreadFile::new(thread, "test"),
        )
        .unwrap();
        (paths, thread)
    }

    #[tokio::test]
    async fn the_same_directory_yields_the_same_writer() {
        let dir = TempDir::new("registry-same");
        let (paths, thread) = thread_paths(&dir);
        let registry = registry();
        let first = registry.writer_for(thread, &paths).unwrap();
        let second = registry.writer_for(thread, &paths).unwrap();
        assert!(first.is_alive());
        assert_eq!(registry.live_count(), 1);
        drop(second);

        first
            .append(crate::store::tests::event_at(thread, 1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_dead_writer_is_replaced() {
        let dir = TempDir::new("registry-dead");
        let (paths, thread) = thread_paths(&dir);
        let registry = registry();
        let first = registry.writer_for(thread, &paths).unwrap();
        first.shutdown().await;
        assert!(!first.is_alive());
        let second = registry.writer_for(thread, &paths).unwrap();
        assert!(second.is_alive());
    }

    #[tokio::test]
    async fn forgetting_removes_the_handle() {
        let dir = TempDir::new("registry-forget");
        let (paths, thread) = thread_paths(&dir);
        let registry = registry();
        registry.writer_for(thread, &paths).unwrap();
        assert!(registry.forget(&paths).is_some());
        assert!(registry.forget(&paths).is_none());
        assert_eq!(registry.live_count(), 0);
    }
}
