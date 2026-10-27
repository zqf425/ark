//! Rebuildable thread index (design 7.4.2).
//!
//! IMPORTANT, v1 scope note: the design document specifies SQLite
//! (`data/index.sqlite`, WAL, FTS5). v1 ships an in-memory index plus an atomic
//! JSON snapshot (`data/index.json`) instead, because:
//!
//! * the crate must not pull in a C-compiling dependency yet;
//! * every guarantee that matters (`indexed_seq` watermark, single-writer
//!   projection, rebuild-from-directory) is expressed on `ThreadIndex` itself,
//!   not on SQL engine behaviour.
//!
//! The `IndexProjector` single-writer semantics are preserved: only the
//! projector mutates the index, and readers see it through an `RwLock`. Swapping
//! this module for `rusqlite` later does not change the port surface or the
//! reconciliation rules (`plan_reconcile` stays as-is).

use crate::domain::diagnostics::{DiagnosticKind, DiagnosticSink, StoreDiagnostic};
use ark_common::{Result, ThreadId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One indexed thread row. Metadata only: event bodies never live in the index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub thread_id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<ThreadId>,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub tokens_total: u64,
    /// Path relative to `data/` so the snapshot survives a moved data dir.
    pub path_rel: String,
    /// How far the projector has consumed this thread's event log.
    #[serde(default)]
    pub indexed_seq: u64,
}

impl IndexEntry {
    pub fn new(thread_id: ThreadId, title: impl Into<String>, path_rel: impl Into<String>) -> Self {
        Self {
            thread_id,
            parent_thread_id: None,
            title: title.into(),
            updated_at: Utc::now(),
            archived: false,
            status: "idle".to_string(),
            tokens_total: 0,
            path_rel: path_rel.into(),
            indexed_seq: 0,
        }
    }
}

/// Whole `data/index.json` document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexSnapshot {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub saved_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub threads: Vec<IndexEntry>,
}

impl Default for IndexSnapshot {
    fn default() -> Self {
        Self {
            version: IndexSnapshot::VERSION,
            saved_at: None,
            threads: Vec::new(),
        }
    }
}

impl IndexSnapshot {
    pub const VERSION: u32 = 1;

    pub fn from_entries(entries: Vec<IndexEntry>) -> Self {
        Self {
            version: Self::VERSION,
            saved_at: Some(Utc::now()),
            threads: entries,
        }
    }
}

/// What the reconciliation pass must do for one thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileAction {
    /// Index and log agree: nothing to do.
    Skip,
    /// Index is behind: replay `(from_seq, to_seq]` only.
    CatchUp { from_seq: u64, to_seq: u64 },
    /// Index is ahead of the log, or the row is inconsistent: rebuild fully.
    FullRebuild,
    /// A directory exists with no index row: insert it.
    Insert,
    /// An index row points at a directory that no longer exists: drop it.
    Remove,
}

/// Facts read from a thread directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskFacts {
    pub max_seq: u64,
}

/// Pure four-branch reconciliation rule (design 7.4.2).
pub fn plan_reconcile(row: Option<&IndexEntry>, disk: Option<DiskFacts>) -> ReconcileAction {
    match (row, disk) {
        (None, None) => ReconcileAction::Skip,
        (None, Some(_)) => ReconcileAction::Insert,
        (Some(_), None) => ReconcileAction::Remove,
        (Some(row), Some(disk)) => {
            if row.indexed_seq == disk.max_seq {
                ReconcileAction::Skip
            } else if row.indexed_seq < disk.max_seq {
                ReconcileAction::CatchUp {
                    from_seq: row.indexed_seq,
                    to_seq: disk.max_seq,
                }
            } else {
                ReconcileAction::FullRebuild
            }
        }
    }
}

/// In-memory index with an atomic JSON snapshot, owned by one projector.
#[derive(Debug, Default)]
pub struct ThreadIndex {
    entries: Vec<IndexEntry>,
    dirty: bool,
}

impl ThreadIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot(snapshot: IndexSnapshot) -> Self {
        Self {
            entries: snapshot.threads,
            dirty: false,
        }
    }

    pub fn snapshot(&self) -> IndexSnapshot {
        IndexSnapshot::from_entries(self.entries.clone())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn get(&self, thread: &ThreadId) -> Option<&IndexEntry> {
        self.entries.iter().find(|entry| &entry.thread_id == thread)
    }

    pub fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }

    /// All threads, active first and then most recently updated.
    pub fn sorted(&self) -> Vec<IndexEntry> {
        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| {
            a.archived
                .cmp(&b.archived)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        sorted
    }

    pub fn insert(&mut self, entry: IndexEntry) {
        match self
            .entries
            .iter_mut()
            .find(|existing| existing.thread_id == entry.thread_id)
        {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
        self.dirty = true;
    }

    pub fn remove(&mut self, thread: &ThreadId) -> Option<IndexEntry> {
        let position = self
            .entries
            .iter()
            .position(|entry| &entry.thread_id == thread)?;
        self.dirty = true;
        Some(self.entries.remove(position))
    }

    pub fn set_watermark(&mut self, thread: &ThreadId, indexed_seq: u64) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| &entry.thread_id == thread)
        {
            if entry.indexed_seq != indexed_seq {
                entry.indexed_seq = indexed_seq;
                self.dirty = true;
            }
        }
    }
}

/// Applies `plan_reconcile` outcomes. One projector per store keeps the index
/// single-writer, exactly as the design requires for the SQLite variant.
pub struct IndexProjector {
    diagnostics: std::sync::Arc<dyn DiagnosticSink>,
}

impl IndexProjector {
    pub fn new(diagnostics: std::sync::Arc<dyn DiagnosticSink>) -> Self {
        Self { diagnostics }
    }

    /// Record the outcome of applying `action` for `thread`.
    pub fn note(&self, thread: ThreadId, action: ReconcileAction) {
        let (kind, detail) = match action {
            ReconcileAction::Skip => return,
            ReconcileAction::CatchUp { from_seq, to_seq } => (
                DiagnosticKind::IndexCaughtUp,
                format!("replayed ({from_seq}, {to_seq}]"),
            ),
            ReconcileAction::FullRebuild => (DiagnosticKind::IndexRebuilt, "full rebuild".into()),
            ReconcileAction::Insert => (DiagnosticKind::IndexInserted, "row inserted".into()),
            ReconcileAction::Remove => (DiagnosticKind::IndexRemoved, "row removed".into()),
        };
        self.diagnostics
            .record(StoreDiagnostic::new(kind, detail).for_thread(thread));
    }
}

/// Persist the snapshot atomically to `data/index.json`.
pub fn save_snapshot(index: &mut ThreadIndex, path: &std::path::Path) -> Result<()> {
    crate::store::fs_util::write_json_atomic(path, &index.snapshot())?;
    index.mark_clean();
    Ok(())
}

/// Load the snapshot, or an empty index when it is missing or damaged.
pub fn load_snapshot(path: &std::path::Path) -> ThreadIndex {
    match crate::store::fs_util::read_json_opt::<IndexSnapshot>(path) {
        Some(snapshot) => ThreadIndex::from_snapshot(snapshot),
        None => ThreadIndex::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::TempDir;

    fn entry(indexed_seq: u64) -> IndexEntry {
        let mut entry = IndexEntry::new(ThreadId::new(), "t", "threads/t");
        entry.indexed_seq = indexed_seq;
        entry
    }

    #[test]
    fn equal_watermarks_skip() {
        let row = entry(5);
        let action = plan_reconcile(Some(&row), Some(DiskFacts { max_seq: 5 }));
        assert_eq!(action, ReconcileAction::Skip);
    }

    #[test]
    fn lagging_index_catches_up_incrementally() {
        let row = entry(3);
        let action = plan_reconcile(Some(&row), Some(DiskFacts { max_seq: 9 }));
        assert_eq!(
            action,
            ReconcileAction::CatchUp {
                from_seq: 3,
                to_seq: 9
            }
        );
    }

    #[test]
    fn running_ahead_forces_a_full_rebuild() {
        let row = entry(9);
        let action = plan_reconcile(Some(&row), Some(DiskFacts { max_seq: 4 }));
        assert_eq!(action, ReconcileAction::FullRebuild);
    }

    #[test]
    fn missing_row_is_inserted_and_missing_directory_is_removed() {
        assert_eq!(
            plan_reconcile(None, Some(DiskFacts { max_seq: 2 })),
            ReconcileAction::Insert
        );
        let row = entry(2);
        assert_eq!(plan_reconcile(Some(&row), None), ReconcileAction::Remove);
        assert_eq!(plan_reconcile(None, None), ReconcileAction::Skip);
    }

    #[test]
    fn projector_emits_a_diagnostic_per_action() {
        use crate::domain::diagnostics::MemoryDiagnostics;
        use std::sync::Arc;

        let sink = Arc::new(MemoryDiagnostics::new());
        let projector = IndexProjector::new(sink.clone());
        let thread = ThreadId::new();
        projector.note(thread, ReconcileAction::Skip);
        assert!(sink.is_empty(), "skip is silent");
        projector.note(thread, ReconcileAction::Insert);
        projector.note(
            thread,
            ReconcileAction::CatchUp {
                from_seq: 1,
                to_seq: 2,
            },
        );
        assert!(sink.has(DiagnosticKind::IndexInserted));
        assert!(sink.has(DiagnosticKind::IndexCaughtUp));
    }

    #[test]
    fn snapshot_roundtrips_through_the_json_file() {
        let dir = TempDir::new("index-snapshot");
        let path = dir.path().join("index.json");
        let mut index = ThreadIndex::new();
        index.insert(entry(7));
        assert!(index.is_dirty());
        save_snapshot(&mut index, &path).unwrap();
        assert!(!index.is_dirty());
        let loaded = load_snapshot(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.entries()[0].indexed_seq, 7);
    }

    #[test]
    fn damaged_snapshot_loads_as_empty() {
        let dir = TempDir::new("index-damaged");
        let path = dir.path().join("index.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(load_snapshot(&path).is_empty());
    }

    #[test]
    fn sorting_puts_active_threads_first() {
        let mut index = ThreadIndex::new();
        let mut archived = entry(0);
        archived.archived = true;
        archived.title = "archived".into();
        index.insert(archived);
        let mut active = entry(0);
        active.title = "active".into();
        index.insert(active);
        assert_eq!(index.sorted()[0].title, "active");
    }
}
