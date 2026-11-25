//! JSON-backed scheduled-task store.
//!
//! One document (`<data_dir>/schedules.json`) holds every task and its bounded
//! run history. That shape is deliberate: schedules are user-authored settings
//! that must survive a restart on their own, and a run fires at most once a
//! minute per task, so a per-task directory tree or an append-only log would buy
//! durability the workload cannot use.
//!
//! ## Write model
//!
//! Reads and rewrites are serialized by one in-process `Mutex`, and every
//! rewrite goes through [`write_json_atomic`] (temp file, fsync, rename). A
//! crash therefore leaves either the previous complete document or the next
//! complete document, never a half-written one. The in-memory snapshot only
//! advances after the write succeeds, so a failed write cannot leave the store
//! claiming state that is not on disk.
//!
//! ## Recovery
//!
//! A missing document is simply an empty table. A *present but unreadable* one
//! is also folded to an empty table, with a warning: refusing to start because a
//! settings file has a stray byte would take the whole runtime down for a
//! feature the user can re-create in ten seconds. The warning is what keeps the
//! loss visible instead of silent.
//!
//! ## What this layer does not do
//!
//! No cadence repair, no `next_run_at` recomputation and no task cap: those live
//! in `ark-schedule`, which the composition root calls before it reaches this
//! store. Persisting exactly what it is handed is what lets the HTTP layer
//! return the same document it just wrote.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use ark_common::{read_json_file_opt, ArkError, Result};
use ark_contract::ScheduledTask;
use ark_ports::ScheduleStore;
use serde::{Deserialize, Serialize};

use crate::store::write_json_atomic;

/// Document name inside the data directory.
pub const SCHEDULE_FILE_NAME: &str = "schedules.json";

/// On-disk document. A wrapper struct (rather than a bare `Vec`) so a future
/// field can be added without changing the file's top-level shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ScheduleFile {
    #[serde(default)]
    tasks: Vec<ScheduledTask>,
}

/// File-backed implementation of [`ScheduleStore`].
pub struct ScheduleFileStore {
    path: PathBuf,
    state: Mutex<ScheduleFile>,
}

impl ScheduleFileStore {
    /// Open `<data_dir>/schedules.json`, treating absent or damaged content as
    /// an empty task table.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(SCHEDULE_FILE_NAME);
        let state = match read_json_file_opt::<ScheduleFile>(&path) {
            Some(state) => state,
            None => {
                // Distinguish "no schedules yet" from "the file is there but
                // useless": the first is normal and silent, the second must be
                // logged or the user's tasks vanish without explanation.
                if path.exists() {
                    tracing::warn!(
                        path = %path.display(),
                        "schedules.json is unreadable; starting from an empty task table"
                    );
                }
                ScheduleFile::default()
            }
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    /// Path of the backing document, for diagnostics and tests.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> MutexGuard<'_, ScheduleFile> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Copy-on-write: the caller mutates the clone, persists it, and only then
    /// swaps it in.
    fn snapshot(state: &ScheduleFile) -> ScheduleFile {
        ScheduleFile {
            tasks: state.tasks.clone(),
        }
    }

    fn persist(&self, next: &ScheduleFile) -> Result<()> {
        write_json_atomic(&self.path, next)
    }

    fn missing(id: &str) -> ArkError {
        ArkError::not_found(format!("scheduled task not found: {id}"))
    }
}

impl ScheduleStore for ScheduleFileStore {
    fn list(&self) -> Result<Vec<ScheduledTask>> {
        let state = self.lock();
        let mut tasks = state.tasks.clone();
        // Newest edit first: the settings list is read as "what changed lately",
        // and stable sorting keeps equal timestamps in insertion order.
        tasks.sort_by_key(|task| std::cmp::Reverse(task.updated_at));
        Ok(tasks)
    }

    fn get(&self, id: &str) -> Result<ScheduledTask> {
        self.lock()
            .tasks
            .iter()
            .find(|task| task.id == id)
            .cloned()
            .ok_or_else(|| Self::missing(id))
    }

    fn insert(&self, mut task: ScheduledTask) -> Result<ScheduledTask> {
        let mut state = self.lock();
        if task.id.trim().is_empty() {
            task.id = format!("sch_{}", uuid::Uuid::new_v4());
        }
        if state.tasks.iter().any(|existing| existing.id == task.id) {
            // `insert` is the create path, so a duplicate id is a bug in the
            // caller (or a retried request), not a request to overwrite: doing
            // so would discard the stored task's history.
            return Err(ArkError::conflict(format!(
                "scheduled task already exists: {}",
                task.id
            )));
        }
        let mut next = Self::snapshot(&state);
        next.tasks.push(task.clone());
        self.persist(&next)?;
        *state = next;
        Ok(task)
    }

    fn replace(&self, task: ScheduledTask) -> Result<ScheduledTask> {
        let mut state = self.lock();
        let index = state
            .tasks
            .iter()
            .position(|existing| existing.id == task.id)
            .ok_or_else(|| Self::missing(&task.id))?;
        let mut next = Self::snapshot(&state);
        next.tasks[index] = task.clone();
        self.persist(&next)?;
        *state = next;
        Ok(task)
    }

    fn update(
        &self,
        id: &str,
        mutate: &mut dyn FnMut(&mut ScheduledTask) -> ark_common::Result<()>,
    ) -> Result<ScheduledTask> {
        let mut state = self.lock();
        let index = state
            .tasks
            .iter()
            .position(|task| task.id == id)
            .ok_or_else(|| Self::missing(id))?;
        let mut candidate = state.tasks[index].clone();
        mutate(&mut candidate)?;
        let mut next = Self::snapshot(&state);
        next.tasks[index] = candidate.clone();
        self.persist(&next)?;
        *state = next;
        Ok(candidate)
    }

    fn delete(&self, id: &str) -> Result<()> {
        let mut state = self.lock();
        let index = state
            .tasks
            .iter()
            .position(|task| task.id == id)
            .ok_or_else(|| Self::missing(id))?;
        let mut next = Self::snapshot(&state);
        next.tasks.remove(index);
        self.persist(&next)?;
        *state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::{RunStatus, ScheduleKind, ScheduledRunRecord, TaskSchedule};
    use chrono::{TimeZone, Utc};

    fn task(id: &str, title: &str) -> ScheduledTask {
        let now = Utc.with_ymd_and_hms(2026, 11, 1, 9, 0, 0).unwrap();
        ScheduledTask {
            id: id.to_string(),
            title: title.to_string(),
            enabled: true,
            prompt: "summarize the inbox".to_string(),
            workspace: None,
            autonomy: None,
            model: None,
            provider_id: None,
            schedule: TaskSchedule {
                kind: ScheduleKind::Daily,
                time_of_day: "09:00".to_string(),
                ..TaskSchedule::default()
            },
            created_at: now,
            updated_at: now,
            last_run: None,
            next_run_at: "2026-11-02T01:00:00+00:00".to_string(),
            history: Vec::new(),
        }
    }

    fn store() -> (tempfile::TempDir, ScheduleFileStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ScheduleFileStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn insert_assigns_an_id_when_the_caller_leaves_it_empty() {
        let (_dir, store) = store();
        let stored = store.insert(task("", "digest")).unwrap();
        assert!(stored.id.starts_with("sch_"), "{}", stored.id);
        assert_eq!(store.get(&stored.id).unwrap().title, "digest");
    }

    #[test]
    fn a_duplicate_insert_is_a_conflict_not_an_overwrite() {
        let (_dir, store) = store();
        store.insert(task("sch_fixed", "first")).unwrap();
        let err = store.insert(task("sch_fixed", "second")).unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::Conflict);
        assert_eq!(store.get("sch_fixed").unwrap().title, "first");
    }

    #[test]
    fn replace_persists_run_bookkeeping() {
        let (_dir, store) = store();
        let mut stored = store.insert(task("sch_1", "digest")).unwrap();
        stored.last_run = Some(ScheduledRunRecord {
            status: RunStatus::Completed,
            message: "done".to_string(),
            thread_id: None,
            started_at: Utc.with_ymd_and_hms(2026, 11, 2, 1, 0, 0).unwrap(),
            finished_at: Some(Utc.with_ymd_and_hms(2026, 11, 2, 1, 0, 5).unwrap()),
        });
        store.replace(stored.clone()).unwrap();

        let reloaded = store.get("sch_1").unwrap();
        let last = reloaded.last_run.unwrap();
        assert_eq!(last.status, RunStatus::Completed);
        assert_eq!(last.message, "done");
    }

    #[test]
    fn replacing_or_deleting_an_absent_task_is_not_found() {
        let (_dir, store) = store();
        let err = store.replace(task("sch_absent", "x")).unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
        let err = store.delete("sch_absent").unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
        let err = store.get("sch_absent").unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
    }

    #[test]
    fn delete_removes_the_task_and_its_history() {
        let (_dir, store) = store();
        store.insert(task("sch_1", "digest")).unwrap();
        store.delete("sch_1").unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn list_returns_the_most_recently_updated_first() {
        let (_dir, store) = store();
        let mut older = task("sch_old", "old");
        older.updated_at = Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap();
        let mut newer = task("sch_new", "new");
        newer.updated_at = Utc.with_ymd_and_hms(2026, 11, 1, 9, 0, 0).unwrap();
        store.insert(older).unwrap();
        store.insert(newer).unwrap();

        let ids: Vec<String> = store.list().unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec!["sch_new".to_string(), "sch_old".to_string()]);
    }

    #[test]
    fn a_damaged_document_falls_back_to_an_empty_table() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(SCHEDULE_FILE_NAME), b"{ not json").unwrap();
        let store = ScheduleFileStore::open(dir.path()).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn a_reopened_store_sees_the_persisted_tasks() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = ScheduleFileStore::open(dir.path()).unwrap();
            store.insert(task("sch_1", "digest")).unwrap();
        }
        let reopened = ScheduleFileStore::open(dir.path()).unwrap();
        assert_eq!(reopened.list().unwrap().len(), 1);
        assert_eq!(
            reopened.get("sch_1").unwrap().next_run_at,
            "2026-11-02T01:00:00+00:00"
        );
    }
}
