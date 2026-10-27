//! Per-thread writer registry: one live actor per thread directory.
//!
//! Design 7.4.3 requires exactly one writer per session directory. The registry
//! is the mapping that enforces it: `writer_for` returns the existing handle if
//! one is alive, and only then opens a new `WriterState`.

use crate::config::StoreLimits;
use crate::domain::diagnostics::DiagnosticSink;
use crate::domain::layout::ThreadPaths;
use crate::store::writer_actor::WriterHandle;
use crate::store::writer_state::WriterState;
use ark_common::{ArkError, Result, ThreadId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Capacity of the per-writer command queue.
const WRITER_QUEUE_CAPACITY: usize = 256;

/// Map from directory key to live writer handle.
pub struct WriterRegistry {
    limits: StoreLimits,
    diagnostics: Arc<dyn DiagnosticSink>,
    inner: Mutex<HashMap<String, WriterHandle>>,
}

impl WriterRegistry {
    pub fn new(limits: StoreLimits, diagnostics: Arc<dyn DiagnosticSink>) -> Self {
        Self {
            limits,
            diagnostics,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Get the live writer for `paths`, opening it when necessary.
    pub fn writer_for(&self, thread: ThreadId, paths: &ThreadPaths) -> Result<WriterHandle> {
        let key = paths.key();
        let mut guard = self.lock()?;
        if let Some(handle) = guard.get(&key) {
            if handle.is_alive() {
                return Ok(handle.clone());
            }
            guard.remove(&key);
        }
        let state = WriterState::open(
            thread,
            paths.clone(),
            self.limits.clone(),
            self.diagnostics.clone(),
        )?;
        let handle = WriterHandle::spawn(state, WRITER_QUEUE_CAPACITY);
        guard.insert(key, handle.clone());
        Ok(handle)
    }

    /// Drop the handle for `paths` without touching the directory.
    pub fn forget(&self, paths: &ThreadPaths) -> Option<WriterHandle> {
        self.lock().ok()?.remove(&paths.key())
    }

    /// All live handles, used to close writers before a directory is removed.
    pub fn drain(&self) -> Vec<WriterHandle> {
        match self.lock() {
            Ok(mut guard) => guard.drain().map(|(_, handle)| handle).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn live_count(&self) -> usize {
        self.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, WriterHandle>>> {
        self.inner
            .lock()
            .map_err(|_| ArkError::internal("writer registry lock poisoned"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::diagnostics::MemoryDiagnostics;
    use crate::store::tests::TempDir;

    fn registry() -> WriterRegistry {
        WriterRegistry::new(StoreLimits::for_tests(), Arc::new(MemoryDiagnostics::new()))
    }

    #[tokio::test]
    async fn the_same_directory_yields_the_same_writer() {
        let dir = TempDir::new("registry-same");
        let paths = ThreadPaths::new(dir.path().join("thread"));
        let registry = registry();
        let thread = ThreadId::new();
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
        let paths = ThreadPaths::new(dir.path().join("thread"));
        let registry = registry();
        let thread = ThreadId::new();
        let first = registry.writer_for(thread, &paths).unwrap();
        first.shutdown().await;
        assert!(!first.is_alive());
        let second = registry.writer_for(thread, &paths).unwrap();
        assert!(second.is_alive());
    }

    #[tokio::test]
    async fn forgetting_removes_the_handle() {
        let dir = TempDir::new("registry-forget");
        let paths = ThreadPaths::new(dir.path().join("thread"));
        let registry = registry();
        registry.writer_for(ThreadId::new(), &paths).unwrap();
        assert!(registry.forget(&paths).is_some());
        assert!(registry.forget(&paths).is_none());
        assert_eq!(registry.live_count(), 0);
    }
}
