//! Child-run index rules (design 7.4.1).
//!
//! `children/runs.jsonl` is authoritative and append-only; `children/index.json`
//! is a cache that `index_from_runs` regenerates. Everything here is pure, so
//! "index equals a fold of the log" is provable without touching a disk.

use ark_common::{ChildId, ThreadId, TurnId};
use ark_ports::ChildRunRecord;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Statuses that mean "the child still owns work". Anything else is terminal.
pub const ACTIVE_STATUSES: [&str; 4] = ["pending", "running", "streaming", "started"];

/// Whether a `ChildRunRecord.status` blocks deleting the parent thread.
pub fn is_active_status(status: &str) -> bool {
    let normalized = status.trim().to_ascii_lowercase();
    ACTIVE_STATUSES.contains(&normalized.as_str())
}

/// One row of `children/index.json` (derived from the run log).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildIndexEntry {
    pub child_id: ChildId,
    pub parent_thread_id: ThreadId,
    pub parent_turn_id: TurnId,
    pub label: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
}

impl ChildIndexEntry {
    pub fn from_record(record: &ChildRunRecord) -> Self {
        Self {
            child_id: record.child_id,
            parent_thread_id: record.parent_thread_id,
            parent_turn_id: record.parent_turn_id,
            label: record.label.clone(),
            status: record.status.clone(),
            created_at: record.created_at,
            ended_at: record.ended_at,
            model: record.model.clone(),
            provider_id: record.provider_id.clone(),
        }
    }

    pub fn is_active(&self) -> bool {
        is_active_status(&self.status)
    }
}

/// Whole `children/index.json` document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChildIndexFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub children: Vec<ChildIndexEntry>,
}

impl ChildIndexFile {
    pub const VERSION: u32 = 1;

    pub fn active(&self) -> Vec<&ChildIndexEntry> {
        self.children
            .iter()
            .filter(|entry| entry.is_active())
            .collect()
    }
}

/// Latest record per child, in order of first appearance.
///
/// The log is append-only, so a later record always supersedes an earlier one
/// with the same `child_id`; order is kept stable so the UI list does not jump.
pub fn fold_child_runs(records: impl IntoIterator<Item = ChildRunRecord>) -> Vec<ChildIndexEntry> {
    let mut order: Vec<ChildId> = Vec::new();
    let mut latest: BTreeMap<ChildId, ChildIndexEntry> = BTreeMap::new();
    for record in records {
        if !latest.contains_key(&record.child_id) {
            order.push(record.child_id);
        }
        latest.insert(record.child_id, ChildIndexEntry::from_record(&record));
    }
    order
        .into_iter()
        .filter_map(|id| latest.remove(&id))
        .collect()
}

/// Rebuild `children/index.json` content from the authoritative run log.
pub fn index_from_runs(records: impl IntoIterator<Item = ChildRunRecord>) -> ChildIndexFile {
    ChildIndexFile {
        version: ChildIndexFile::VERSION,
        children: fold_child_runs(records),
    }
}

/// Where a child directory lives, derived from the authoritative run log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildLocation {
    pub parent_thread_id: ThreadId,
    pub rel_path: String,
}

impl ChildLocation {
    pub fn new(parent_thread_id: ThreadId, rel_path: impl Into<String>) -> Self {
        Self {
            parent_thread_id,
            rel_path: rel_path.into(),
        }
    }
}

/// In-memory `child_id -> (parent_id, rel_path)` map (design 7.4.1).
#[derive(Debug, Clone, Default)]
pub struct ChildLocator {
    inner: BTreeMap<ChildId, ChildLocation>,
}

impl ChildLocator {
    pub fn insert(&mut self, child: ChildId, location: ChildLocation) {
        self.inner.insert(child, location);
    }

    pub fn get(&self, child: &ChildId) -> Option<&ChildLocation> {
        self.inner.get(child)
    }

    pub fn remove(&mut self, child: &ChildId) -> Option<ChildLocation> {
        self.inner.remove(child)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ChildId, &ChildLocation)> {
        self.inner.iter()
    }

    /// Drop every child that belongs to `parent` (used when a parent is deleted).
    pub fn retain_parent(&mut self, parent: ThreadId) {
        self.inner
            .retain(|_, location| location.parent_thread_id != parent);
    }

    /// Register every authoritative record found under `parent`.
    ///
    /// Returns the ids whose previous location disagreed with the log, so the
    /// caller can emit an `index_repaired` diagnostic.
    pub fn absorb(
        &mut self,
        parent: ThreadId,
        records: &[ChildRunRecord],
        rel_of: impl Fn(&ChildId) -> String,
    ) -> Vec<ChildId> {
        let mut repaired = Vec::new();
        for record in records {
            let location = ChildLocation::new(parent, rel_of(&record.child_id));
            match self.inner.get(&record.child_id) {
                Some(existing) if existing == &location => {}
                Some(_) => {
                    repaired.push(record.child_id);
                    self.inner.insert(record.child_id, location);
                }
                None => {
                    self.inner.insert(record.child_id, location);
                }
            }
        }
        repaired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(child: ChildId, parent: ThreadId, status: &str) -> ChildRunRecord {
        let mut record = ChildRunRecord::queued(child, parent, TurnId::new(), "scan", Utc::now());
        record.status = status.into();
        record
    }

    #[test]
    fn active_statuses_are_recognized_case_insensitively() {
        assert!(is_active_status("running"));
        assert!(is_active_status(" Running "));
        assert!(!is_active_status("completed"));
        assert!(!is_active_status("failed"));
    }

    #[test]
    fn fold_keeps_the_last_record_and_first_order() {
        let parent = ThreadId::new();
        let a = ChildId::new();
        let b = ChildId::new();
        let entries = fold_child_runs(vec![
            record(a, parent, "running"),
            record(b, parent, "running"),
            record(a, parent, "completed"),
        ]);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].child_id, a);
        assert_eq!(entries[0].status, "completed");
        assert!(!entries[0].is_active());
        assert_eq!(entries[1].child_id, b);
    }

    #[test]
    fn index_is_a_pure_fold_of_the_run_log() {
        let parent = ThreadId::new();
        let child = ChildId::new();
        let index = index_from_runs(vec![
            record(child, parent, "running"),
            record(child, parent, "completed"),
        ]);
        assert_eq!(index.version, ChildIndexFile::VERSION);
        assert_eq!(index.children.len(), 1);
        assert!(index.active().is_empty());
    }

    #[test]
    fn locator_reports_disagreements() {
        let parent = ThreadId::new();
        let child = ChildId::new();
        let mut locator = ChildLocator::default();
        let records = vec![record(child, parent, "running")];

        let first = locator.absorb(parent, &records, |id| {
            format!("threads/{parent}/children/{id}")
        });
        assert!(first.is_empty(), "first insert is not a repair");

        let other_parent = ThreadId::new();
        let second = locator.absorb(other_parent, &records, |id| {
            format!("threads/{other_parent}/children/{id}")
        });
        assert_eq!(second, vec![child]);
        assert_eq!(locator.get(&child).unwrap().parent_thread_id, other_parent);
    }
}
