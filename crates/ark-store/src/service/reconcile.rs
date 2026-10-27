//! Startup reconciliation: index versus event-log watermarks (design 7.4.2).
//!
//! The four branches are decided by the pure `plan_reconcile` function; this
//! module only carries out the decision and reports what it did. Running it is
//! always safe: every branch is idempotent, and the directory stays the system
//! of record even if the index is deleted entirely.

use crate::domain::diagnostics::StoreDiagnostic;
use crate::domain::layout::{names, ThreadPaths};
use crate::domain::session_state::SessionState;
use crate::domain::thread_file::{self, ThreadFile};
use crate::service::store::FileSessionStore;
use crate::store::child_store::{absorb_parent_blocking, sync_child_index_blocking};
use crate::store::event_log::read_log_blocking;
use crate::store::fs_util::{list_dirs, save_json_if_changed};
use crate::store::index::{
    plan_reconcile, save_snapshot, DiskFacts, IndexEntry, IndexProjector, ReconcileAction,
};
use crate::store::items_log::{rebuild_items_blocking, thread_usage_blocking};
use ark_common::{Result, ThreadId};

/// What one reconciliation pass did, per branch.
#[derive(Debug, Default, Clone)]
pub struct ReconcileReport {
    /// Thread directories found on disk.
    pub scanned: usize,
    /// Threads whose `indexed_seq` already matched the log.
    pub skipped: usize,
    /// Threads whose index was behind and only needed the metadata refreshed.
    pub caught_up: Vec<ThreadId>,
    /// Threads rebuilt from scratch because the index was ahead or damaged.
    pub rebuilt: Vec<ThreadId>,
    /// Thread directories that had no index row at all.
    pub inserted: Vec<ThreadId>,
    /// Index rows whose directory no longer exists.
    pub removed: Vec<ThreadId>,
    /// Child locator entries that disagreed with `children/runs.jsonl`.
    pub children_repaired: usize,
}

impl ReconcileReport {
    pub fn changed(&self) -> usize {
        self.caught_up.len() + self.rebuilt.len() + self.inserted.len() + self.removed.len()
    }

    /// One-line summary for the startup health report.
    pub fn describe(&self) -> String {
        format!(
            "index sync: scanned={} skipped={} caught_up={} rebuilt={} inserted={} \
             removed={} children_repaired={}",
            self.scanned,
            self.skipped,
            self.caught_up.len(),
            self.rebuilt.len(),
            self.inserted.len(),
            self.removed.len(),
            self.children_repaired
        )
    }
}

/// One thread discovered on disk, with the facts the planner needs.
struct DiskThread {
    thread_id: ThreadId,
    paths: ThreadPaths,
    file: ThreadFile,
    facts: DiskFacts,
}

/// Run the full reconciliation pass and persist the index snapshot.
pub fn reconcile_index(store: &FileSessionStore) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();
    let projector = IndexProjector::new(store.diagnostics());
    let disk = scan_threads(store)?;
    report.scanned = disk.len();

    // The row is read under a short-lived lock: `apply_action` takes the same
    // lock to insert its result, and `std::sync::Mutex` is not re-entrant.
    for item in &disk {
        let row = store.lock_index()?.get(&item.thread_id).cloned();
        let action = plan_reconcile(row.as_ref(), Some(item.facts));
        apply_action(store, &row, item, action, &mut report)?;
    }

    sweep_missing_rows(store, &disk, &mut report, &projector)?;

    let repaired = sweep_children(store, &disk)?;
    report.children_repaired = repaired;

    let mut index = store.lock_index()?;
    if index.is_dirty() {
        save_snapshot(&mut index, &store.data_paths().index_file())?;
    }
    Ok(report)
}

/// Index rows whose directory vanished are dropped (design 7.4.2, branch 4).
fn sweep_missing_rows(
    store: &FileSessionStore,
    disk: &[DiskThread],
    report: &mut ReconcileReport,
    projector: &IndexProjector,
) -> Result<()> {
    let known: Vec<ThreadId> = {
        let index = store.lock_index()?;
        index
            .entries()
            .iter()
            .map(|entry| entry.thread_id)
            .collect()
    };
    for thread in known {
        if disk.iter().any(|item| item.thread_id == thread) {
            continue;
        }
        if store.index_remove(thread)?.is_some() {
            projector.note(thread, ReconcileAction::Remove);
            report.removed.push(thread);
        }
    }
    Ok(())
}

fn apply_action(
    store: &FileSessionStore,
    row: &Option<IndexEntry>,
    item: &DiskThread,
    action: ReconcileAction,
    report: &mut ReconcileReport,
) -> Result<()> {
    match action {
        ReconcileAction::Skip => report.skipped += 1,
        ReconcileAction::CatchUp { .. } => {
            let entry = rebuild_entry(store, item, false)?;
            store.lock_index()?.insert(entry);
            store.diagnostics().record(
                StoreDiagnostic::new(
                    crate::domain::diagnostics::DiagnosticKind::IndexCaughtUp,
                    format!("metadata refreshed to seq {}", item.facts.max_seq),
                )
                .for_thread(item.thread_id),
            );
            report.caught_up.push(item.thread_id);
        }
        ReconcileAction::FullRebuild => {
            // Rebuild the derived files too: an index that ran ahead usually
            // means the projections were written against a longer log.
            rebuild_items_blocking(&item.paths)?;
            let entry = rebuild_entry(store, item, true)?;
            store.lock_index()?.insert(entry);
            save_json_if_changed(
                &item.paths.session_json(),
                &SessionState::replay(&read_log_blocking(&item.paths)?.events),
            )?;
            store.diagnostics().record(
                StoreDiagnostic::new(
                    crate::domain::diagnostics::DiagnosticKind::IndexRebuilt,
                    format!("full rebuild from seq 0..{}", item.facts.max_seq),
                )
                .for_thread(item.thread_id),
            );
            report.rebuilt.push(item.thread_id);
        }
        ReconcileAction::Insert => {
            let entry = rebuild_entry(store, item, true)?;
            store.lock_index()?.insert(entry);
            store.diagnostics().record(
                StoreDiagnostic::new(
                    crate::domain::diagnostics::DiagnosticKind::IndexInserted,
                    format!("row inserted at seq {}", item.facts.max_seq),
                )
                .for_thread(item.thread_id),
            );
            report.inserted.push(item.thread_id);
        }
        ReconcileAction::Remove => {
            // Already handled by `sweep_missing_rows`; keep the match total.
            let _ = row;
        }
    }
    Ok(())
}

/// Build a fresh index row from the directory (metadata only, never bodies).
fn rebuild_entry(
    store: &FileSessionStore,
    item: &DiskThread,
    recompute_usage: bool,
) -> Result<IndexEntry> {
    let mut entry = IndexEntry::new(
        item.thread_id,
        item.file.title.clone(),
        store.data_paths().thread_rel(&item.thread_id),
    );
    entry.parent_thread_id = item.file.parent_thread_id;
    entry.updated_at = item.file.updated_at;
    entry.archived = item.file.archived;
    entry.indexed_seq = item.facts.max_seq;
    entry.status = crate::store::fs_util::read_json_opt::<SessionState>(&item.paths.session_json())
        .map(|state| state.status.as_str().to_string())
        .unwrap_or_else(|| "idle".to_string());
    if recompute_usage {
        entry.tokens_total = thread_usage_blocking(&item.paths)
            .map(|usage| usage.total_tokens)
            .unwrap_or(0);
    }
    Ok(entry)
}

/// Scan `data/threads/*` for real thread directories (a `thread.json` exists).
fn scan_threads(store: &FileSessionStore) -> Result<Vec<DiskThread>> {
    let mut found = Vec::new();
    for dir in list_dirs(&store.data_paths().threads_dir()) {
        if dir
            .file_name()
            .map(|name| name.to_string_lossy().starts_with(names::DELETING_PREFIX))
            .unwrap_or(false)
        {
            continue;
        }
        let paths = ThreadPaths::new(dir);
        let Some(file) = thread_file::read(&paths) else {
            continue;
        };
        let parsed = read_log_blocking(&paths)?;
        found.push(DiskThread {
            thread_id: file.thread_id,
            paths,
            file,
            facts: DiskFacts {
                max_seq: parsed.max_seq,
            },
        });
    }
    Ok(found)
}

/// Rebuild the child locator and every `children/index.json` from the run logs.
fn sweep_children(store: &FileSessionStore, disk: &[DiskThread]) -> Result<usize> {
    let mut repaired = 0usize;
    for item in disk {
        // Lock order is always parent log -> locator. Do not hold the global
        // locator while waiting for a parent lock: append/delete take the same
        // locks in this order.
        let disagreements = store.with_child_log_lock(item.thread_id, || {
            let disagreements = {
                let mut locator = store
                    .children
                    .lock()
                    .map_err(|_| ark_common::ArkError::internal("child locator lock poisoned"))?;
                let before = locator.absorb(item.thread_id, &[], |_| String::new()).len();
                let _ = before;
                absorb_parent_blocking(&mut locator, item.thread_id, &item.paths)
            };
            let entries = sync_child_index_blocking(
                &item.paths,
                item.thread_id,
                store.diagnostics().as_ref(),
            )?;
            let _ = entries;
            Ok(disagreements)
        })?;
        if !disagreements.is_empty() {
            repaired += disagreements.len();
            for child in disagreements {
                store.diagnostics().record(
                    StoreDiagnostic::new(
                        crate::domain::diagnostics::DiagnosticKind::IndexRepaired,
                        format!("locator re-pointed at children/runs.jsonl for {child}"),
                    )
                    .for_thread(item.thread_id),
                );
            }
        }
    }
    Ok(repaired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{StoreConfig, StoreLimits};
    use crate::domain::diagnostics::MemoryDiagnostics;
    use crate::service::store::FileSessionStore;
    use crate::store::tests::{event_at, TempDir};
    use ark_ports::SessionStore;
    use std::sync::Arc;

    async fn store_with_thread(dir: &TempDir) -> (FileSessionStore, ThreadId) {
        let store = FileSessionStore::with_diagnostics(
            StoreConfig::new(dir.path()).with_limits(StoreLimits::for_tests()),
            Arc::new(MemoryDiagnostics::new()),
        )
        .unwrap();
        let meta = store.create_thread("t").await.unwrap();
        (store, meta.thread_id)
    }

    #[tokio::test]
    async fn equal_watermarks_skip_everything() {
        let dir = TempDir::new("reconcile-skip");
        let (store, thread) = store_with_thread(&dir).await;
        store
            .append_event(thread, event_at(thread, 1))
            .await
            .unwrap();
        store.index_set_watermark(thread, 1);
        let report = store.reconcile().unwrap();
        assert_eq!(report.skipped, 1);
        assert!(report.caught_up.is_empty());
        assert!(report.rebuilt.is_empty());
    }

    #[tokio::test]
    async fn a_lagging_index_is_caught_up_without_rebuilding() {
        let dir = TempDir::new("reconcile-catchup");
        let (store, thread) = store_with_thread(&dir).await;
        for _ in 0..3 {
            store
                .append_event(thread, event_at(thread, 1))
                .await
                .unwrap();
        }
        store.index_set_watermark(thread, 1);
        let report = store.reconcile().unwrap();
        assert_eq!(report.caught_up, vec![thread]);
        let index = store.lock_index().unwrap();
        assert_eq!(index.get(&thread).unwrap().indexed_seq, 3);
    }

    #[tokio::test]
    async fn an_index_ahead_of_the_log_forces_a_full_rebuild() {
        let dir = TempDir::new("reconcile-rebuild");
        let (store, thread) = store_with_thread(&dir).await;
        store
            .append_event(thread, event_at(thread, 1))
            .await
            .unwrap();
        store.index_set_watermark(thread, 99);
        let report = store.reconcile().unwrap();
        assert_eq!(report.rebuilt, vec![thread]);
        let index = store.lock_index().unwrap();
        assert_eq!(index.get(&thread).unwrap().indexed_seq, 1);
    }

    #[tokio::test]
    async fn a_missing_row_is_inserted_from_the_directory() {
        let dir = TempDir::new("reconcile-insert");
        let (store, thread) = store_with_thread(&dir).await;
        store
            .append_event(thread, event_at(thread, 1))
            .await
            .unwrap();
        store.index_remove(thread).unwrap();
        let report = store.reconcile().unwrap();
        assert_eq!(report.inserted, vec![thread]);
        assert!(store.lock_index().unwrap().get(&thread).is_some());
    }

    #[tokio::test]
    async fn a_row_without_a_directory_is_removed() {
        let dir = TempDir::new("reconcile-remove");
        let (store, thread) = store_with_thread(&dir).await;
        std::fs::remove_dir_all(store.thread_paths(thread).dir()).unwrap();
        let report = store.reconcile().unwrap();
        assert_eq!(report.removed, vec![thread]);
        assert!(store.lock_index().unwrap().get(&thread).is_none());
    }
}
