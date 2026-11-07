//! Child-run index rules (design 7.4.1).
//!
//! `children/runs.jsonl` is authoritative and append-only; `children/index.json`
//! is a cache that `index_from_runs` regenerates. Everything here is pure, so
//! "index equals a fold of the log" is provable without touching a disk.

use ark_common::{ArkError, ChildId, Result, ThreadId, TurnId};
use ark_ports::ChildRunRecord;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Statuses that mean "the child still owns work". Anything else is terminal.
///
/// Whether a run state blocks deleting the parent thread: anything that still
/// holds (or waits for) a concurrency slot.
pub fn is_active_status(status: ark_common::ChildRunStatus) -> bool {
    matches!(
        status,
        ark_common::ChildRunStatus::Queued | ark_common::ChildRunStatus::Running
    )
}

/// One row of `children/index.json` (derived from the run log).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildIndexEntry {
    pub child_id: ChildId,
    pub parent_thread_id: ThreadId,
    pub parent_turn_id: TurnId,
    pub label: String,
    pub status: ark_common::ChildRunStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
}

impl ChildIndexEntry {
    pub fn from_record(record: &ChildRunRecord) -> Self {
        Self {
            child_id: record.child_id,
            parent_thread_id: record.parent_thread_id,
            parent_turn_id: record.parent_turn_id,
            label: record.label.clone(),
            status: record.status,
            created_at: record.created_at,
            ended_at: record.ended_at,
            model: record.model.clone(),
            provider_id: record.provider_id.clone(),
            autonomy: record.autonomy.clone(),
        }
    }

    pub fn is_active(&self) -> bool {
        is_active_status(self.status)
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

    /// Reject a child id already claimed at a different location.
    pub fn ensure_available(&self, child: ChildId, expected: &ChildLocation) -> Result<()> {
        let Some(existing) = self.inner.get(&child) else {
            return Ok(());
        };
        if existing == expected {
            return Ok(());
        }
        Err(ArkError::conflict(format!(
            "child {child} is already owned by parent {}, not {}",
            existing.parent_thread_id, expected.parent_thread_id
        ))
        .with_detail(
            serde_json::json!({
                "child_id": child,
                "existing_parent_thread_id": existing.parent_thread_id,
                "requested_parent_thread_id": expected.parent_thread_id,
            })
            .to_string(),
        ))
    }

    /// Claim a child id without allowing scan or call order to change ownership.
    pub fn claim(&mut self, child: ChildId, location: ChildLocation) -> Result<()> {
        self.ensure_available(child, &location)?;
        self.inner.insert(child, location);
        Ok(())
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
    /// A child id is globally unique: a second parent is corruption, not an
    /// index disagreement that scan order may repair by overwriting.
    pub fn absorb(
        &mut self,
        parent: ThreadId,
        records: &[ChildRunRecord],
        rel_of: impl Fn(&ChildId) -> String,
    ) -> Result<()> {
        for record in records {
            let location = ChildLocation::new(parent, rel_of(&record.child_id));
            self.claim(record.child_id, location)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        child: ChildId,
        parent: ThreadId,
        status: ark_common::ChildRunStatus,
    ) -> ChildRunRecord {
        let mut record = ChildRunRecord::queued(child, parent, TurnId::new(), "scan", Utc::now());
        record.status = status;
        record
    }

    #[test]
    fn only_slot_holding_states_block_deletion() {
        assert!(is_active_status(ark_common::ChildRunStatus::Running));
        assert!(!is_active_status(ark_common::ChildRunStatus::Completed));
        assert!(!is_active_status(ark_common::ChildRunStatus::Failed));
    }

    #[test]
    fn a_queued_child_is_active_so_it_blocks_deleting_its_parent() {
        // `queued` means "accepted, not started yet". Treating it as terminal
        // would let the parent directory be deleted while the run still holds a
        // reservation, and the child would then write into nothing (P1-1).
        assert!(is_active_status(ark_common::ChildRunStatus::Queued));
        let parent = ThreadId::new();
        let child = ChildId::new();
        let queued = record(child, parent, ark_common::ChildRunStatus::Queued);
        assert!(queued.is_active());
        let entry = ChildIndexEntry::from_record(&queued);
        assert!(entry.is_active());
    }

    #[test]
    fn the_store_and_the_record_agree_about_every_known_status() {
        // Two independent liveness rules existed (this one and
        // `ChildRunRecord::is_active`); they must not be able to drift again.
        let parent = ThreadId::new();
        for status in ark_common::ChildRunStatus::ALL {
            let record = record(ChildId::new(), parent, status);
            let entry = ChildIndexEntry::from_record(&record);
            assert_eq!(
                entry.is_active(),
                record.is_active(),
                "status `{status:?}` disagrees between index and record"
            );
        }
    }

    #[test]
    fn fold_keeps_the_last_record_and_first_order() {
        let parent = ThreadId::new();
        let a = ChildId::new();
        let b = ChildId::new();
        let entries = fold_child_runs(vec![
            record(a, parent, ark_common::ChildRunStatus::Running),
            record(b, parent, ark_common::ChildRunStatus::Running),
            record(a, parent, ark_common::ChildRunStatus::Completed),
        ]);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].child_id, a);
        assert_eq!(entries[0].status, ark_common::ChildRunStatus::Completed);
        assert!(!entries[0].is_active());
        assert_eq!(entries[1].child_id, b);
    }

    #[test]
    fn index_is_a_pure_fold_of_the_run_log() {
        let parent = ThreadId::new();
        let child = ChildId::new();
        let index = index_from_runs(vec![
            record(child, parent, ark_common::ChildRunStatus::Running),
            record(child, parent, ark_common::ChildRunStatus::Completed),
        ]);
        assert_eq!(index.version, ChildIndexFile::VERSION);
        assert_eq!(index.children.len(), 1);
        assert!(index.active().is_empty());
    }

    #[test]
    fn locator_rejects_a_second_parent_without_changing_ownership() {
        let parent = ThreadId::new();
        let child = ChildId::new();
        let mut locator = ChildLocator::default();
        let records = vec![record(child, parent, ark_common::ChildRunStatus::Running)];

        locator
            .absorb(parent, &records, |id| {
                format!("threads/{parent}/children/{id}")
            })
            .unwrap();

        let other_parent = ThreadId::new();
        let error = locator
            .absorb(other_parent, &records, |id| {
                format!("threads/{other_parent}/children/{id}")
            })
            .unwrap_err();
        assert_eq!(error.code, ark_common::ArkErrorCode::Conflict);
        assert_eq!(locator.get(&child).unwrap().parent_thread_id, parent);
    }
}
