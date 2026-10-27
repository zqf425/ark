//! Child-run service: one parent's `children/` directory (design 7.4.1).
//!
//! `runs.jsonl` is authoritative, `index.json` is refreshed from it, and the
//! locator is the only place a `child_id -> parent` lookup is answered.

use crate::domain::child_index::{ChildIndexEntry, ChildIndexFile};
use crate::domain::layout::ThreadPaths;
use crate::domain::thread_file::ThreadFile;
use crate::service::delegation::DelegationPolicy;
use crate::service::store::FileSessionStore;
use crate::store::child_store::{
    active_child_runs_blocking, begin_children_delete_blocking, finish_children_delete_blocking,
    pending_tombstones_blocking, read_child_index_blocking, read_child_runs_blocking,
    sync_child_index_blocking,
};
use crate::store::fs_util::{ensure_dir, write_json_atomic};
use ark_common::{ArkError, ChildId, Result, ThreadId};
use ark_ports::ChildRunRecord;

/// Child-run operations scoped to one parent thread.
pub struct ChildStore<'a> {
    store: &'a FileSessionStore,
    parent: ThreadId,
}

impl<'a> ChildStore<'a> {
    pub fn new(store: &'a FileSessionStore, parent: ThreadId) -> Self {
        Self { store, parent }
    }

    pub fn parent(&self) -> ThreadId {
        self.parent
    }

    pub fn parent_paths(&self) -> ThreadPaths {
        self.store.thread_paths(self.parent)
    }

    pub fn child_paths(&self, child: ChildId) -> ThreadPaths {
        self.store.data_paths().child(&self.parent, &child)
    }

    /// Authoritative records, straight from `children/runs.jsonl`.
    pub fn runs(&self) -> Vec<ChildRunRecord> {
        read_child_runs_blocking(&self.parent_paths())
    }

    /// Latest state per child, derived from the authoritative log.
    pub fn entries(&self) -> Vec<ChildIndexEntry> {
        crate::store::child_store::child_entries_blocking(&self.parent_paths())
    }

    /// The derived cache (never used for authorization).
    pub fn index(&self) -> Option<ChildIndexFile> {
        read_child_index_blocking(&self.parent_paths())
    }

    /// Children that still own work; the delete gate reads this.
    pub fn active(&self) -> Vec<ChildIndexEntry> {
        active_child_runs_blocking(&self.parent_paths())
    }

    /// Rebuild `index.json` from the log and return the authoritative entries.
    pub fn sync_index(&self) -> Result<Vec<ChildIndexEntry>> {
        self.store.with_child_log_lock(self.parent, || {
            sync_child_index_blocking(
                &self.parent_paths(),
                self.parent,
                self.store.diagnostics().as_ref(),
            )
        })
    }

    /// Append one authoritative record, then refresh the cache and the locator.
    pub fn append(&self, record: ChildRunRecord) -> Result<()> {
        if record.parent_thread_id != self.parent {
            return Err(ArkError::validation(format!(
                "child run {} claims parent {}, expected {}",
                record.child_id, record.parent_thread_id, self.parent
            )));
        }
        // Keep one production append path so every caller shares the same
        // parent-scoped log lock.
        self.store.append_child_run_blocking(&record)
    }

    /// Enforce `DelegationPolicy::max_depth` for this parent.
    pub fn ensure_can_delegate(&self) -> Result<()> {
        let policy = DelegationPolicy::default();
        let depth = u8::from(self.store.prefix_is_child(self.parent));
        policy
            .check(self.parent, depth)
            .map_err(|(thread, error)| error.into_ark_error(thread, depth > 0))
    }

    /// Create the child directory with its `thread.json`.
    pub fn create_child_dir(&self, child: ChildId, label: &str) -> Result<ThreadPaths> {
        self.ensure_can_delegate()?;
        let paths = self.child_paths(child);
        ensure_dir(paths.dir())?;
        ensure_dir(&paths.attachments_dir())?;
        ensure_dir(&paths.runs_dir())?;
        ensure_dir(&paths.children_dir())?;
        let mut file = ThreadFile::new(
            ThreadId::from_uuid(child.as_uuid()),
            format!("{label} (child of {})", self.parent),
        );
        file.parent_thread_id = Some(self.parent);
        write_json_atomic(&paths.thread_json(), &file)?;
        self.store.note_child_location(self.parent, child);
        Ok(paths)
    }

    /// Rename `children/` to a tombstone so a crash is recognizable.
    ///
    /// Returns `None` when the parent has no children directory at all.
    pub fn tombstone_children(&self) -> Option<ThreadPaths> {
        let children = self.parent_paths().children_dir();
        if !children.exists() {
            return None;
        }
        begin_children_delete_blocking(&self.parent_paths()).ok()
    }

    /// Delete tombstones left behind by an interrupted delete.
    pub fn cleanup_tombstones(&self) -> usize {
        let mut removed = 0usize;
        for tombstone in pending_tombstones_blocking(&self.parent_paths()) {
            if finish_children_delete_blocking(&tombstone).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::store::tests::test_store;
    use crate::store::tests::TempDir;
    use ark_common::{ArkErrorCode, TurnId};
    use ark_ports::SessionStore;
    use chrono::Utc;

    fn record(parent: ThreadId, child: ChildId, status: &str) -> ChildRunRecord {
        let mut record = ChildRunRecord::queued(child, parent, TurnId::new(), "scan", Utc::now());
        record.status = status.into();
        record
    }

    #[tokio::test]
    async fn child_directory_is_inlined_under_the_parent() {
        let dir = TempDir::new("child-service-inline");
        let store = test_store(&dir);
        let parent = store.create_thread("parent").await.unwrap();
        let child = ChildId::new();
        let paths = store
            .child_store(parent.thread_id)
            .create_child_dir(child, "scan")
            .unwrap();
        assert!(paths.thread_json().is_file());
        assert!(paths
            .dir()
            .ends_with(format!("{}/children/{child}", parent.thread_id)));
        let file = crate::domain::thread_file::read(&paths).unwrap();
        assert_eq!(file.parent_thread_id, Some(parent.thread_id));
    }

    #[tokio::test]
    async fn a_child_run_cannot_delegate_further() {
        let dir = TempDir::new("child-service-depth");
        let store = test_store(&dir);
        let parent = store.create_thread("parent").await.unwrap();
        let child = ChildId::new();
        store
            .child_store(parent.thread_id)
            .create_child_dir(child, "scan")
            .unwrap();
        let grandchild_parent = ThreadId::from_uuid(child.as_uuid());
        let err = store
            .child_store(grandchild_parent)
            .create_child_dir(ChildId::new(), "deep")
            .unwrap_err();
        assert_eq!(err.code, ArkErrorCode::ValidationError);
        assert!(err.message.contains("depth limit is 1"), "{}", err.message);
    }

    #[tokio::test]
    async fn appending_a_foreign_record_is_rejected() {
        let dir = TempDir::new("child-service-foreign");
        let store = test_store(&dir);
        let parent = store.create_thread("parent").await.unwrap();
        let other = store.create_thread("other").await.unwrap();
        let err = store
            .child_store(parent.thread_id)
            .append(record(other.thread_id, ChildId::new(), "running"))
            .unwrap_err();
        assert_eq!(err.code, ArkErrorCode::ValidationError);
    }

    #[tokio::test]
    async fn tombstone_children_removes_the_tree_when_finished() {
        let dir = TempDir::new("child-service-tombstone");
        let store = test_store(&dir);
        let parent = store.create_thread("parent").await.unwrap();
        let child = ChildId::new();
        store
            .child_store(parent.thread_id)
            .create_child_dir(child, "scan")
            .unwrap();
        let tombstone = store
            .child_store(parent.thread_id)
            .tombstone_children()
            .expect("tombstone");
        assert!(crate::store::child_store::is_deleting_dir(
            &tombstone.dir().file_name().unwrap().to_string_lossy()
        ));
        assert!(!store
            .data_paths()
            .child(&parent.thread_id, &child)
            .dir()
            .exists());
        assert_eq!(store.child_store(parent.thread_id).cleanup_tombstones(), 1);
        assert!(!tombstone.dir().exists());
    }
}
