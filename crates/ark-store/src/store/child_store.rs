//! Child-run storage: `children/runs.jsonl` is authoritative (design 7.4.1).
//!
//! `children/index.json` is a derived cache. Ownership checks read the run log,
//! never the cache, so a hand-edited index cannot grant access to a child that
//! the log does not claim.

use crate::domain::child_index::{
    fold_child_runs, index_from_runs, ChildIndexEntry, ChildIndexFile, ChildLocator,
};
use crate::domain::diagnostics::{DiagnosticKind, DiagnosticSink, StoreDiagnostic};
use crate::domain::layout::{names, ThreadPaths};
use crate::store::event_log::{append_text, encode_line};
use crate::store::fs_util::{
    ensure_dir, is_file, list_dirs, read_json_opt, remove_dir_all, rename_dir, write_json_atomic,
};
use ark_common::{ArkError, ChildId, Result, ThreadId};
use ark_ports::ChildRunRecord;

/// Append one authoritative child-run record.
///
/// The tail is repaired first. A process killed mid-write leaves a partial JSON
/// line with no trailing newline; appending directly would glue the next record
/// onto those bytes, making the whole line unparseable — so the new record would
/// be lost and a queued/running child would silently disappear from the log
/// (P1-2). Only the **last** line is eligible: damage in the middle is real
/// corruption and must stay visible rather than be truncated away.
pub fn append_child_run_blocking(
    parent_paths: &ThreadPaths,
    record: &ChildRunRecord,
    fsync: bool,
) -> Result<()> {
    ensure_dir(&parent_paths.children_dir())?;
    repair_child_runs_tail_blocking(parent_paths)?;
    append_text(&parent_paths.children_runs(), &encode_line(record)?, fsync)
}

/// Cut a torn final line from `children/runs.jsonl`, if there is one.
///
/// Returns the number of bytes removed (0 when the log ends cleanly), which the
/// reconciliation path reports as a repair rather than hiding it.
pub fn repair_child_runs_tail_blocking(parent_paths: &ThreadPaths) -> Result<u64> {
    let path = parent_paths.children_runs();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(ArkError::store(format!(
                "read child run log failed for {}: {err}",
                path.display()
            )))
        }
    };
    // `None` when the file ends with a newline, i.e. the last record is whole.
    let keep = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(pos) => {
            let complete = pos + 1;
            let tail = &bytes[complete..];
            if tail.is_empty() || serde_json::from_slice::<ChildRunRecord>(tail).is_ok() {
                // Either clean, or a complete final record that merely lacks its
                // newline; `append_text` adds the separator in that case.
                if tail.is_empty() {
                    return Ok(0);
                }
                bytes.len() as u64
            } else {
                complete as u64
            }
        }
        // No newline at all: the single line is either a complete record or torn.
        None => {
            if serde_json::from_slice::<ChildRunRecord>(&bytes).is_ok() {
                bytes.len() as u64
            } else {
                0
            }
        }
    };
    if keep == bytes.len() as u64 {
        return Ok(0);
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(|err| ArkError::store(format!("open child log for repair failed: {err}")))?;
    file.set_len(keep)
        .map_err(|err| ArkError::store(format!("truncate child log failed: {err}")))?;
    file.sync_all()
        .map_err(|err| ArkError::store(format!("truncate child log fsync failed: {err}")))?;
    Ok(bytes.len() as u64 - keep)
}

/// Read every parsable record from `children/runs.jsonl`.
///
/// A damaged line is skipped: the log stays append-only and a single bad byte
/// cannot make the whole thread's children disappear. Decoding follows the
/// shared policy so a BOM cannot hide the first record.
pub fn read_child_runs_blocking(parent_paths: &ThreadPaths) -> Vec<ChildRunRecord> {
    let raw = match ark_common::read_text_opt(&parent_paths.children_runs()) {
        Some(raw) => raw,
        None => return Vec::new(),
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<ChildRunRecord>(line).ok())
        .collect()
}

/// Latest state per child, derived from the authoritative log.
pub fn child_entries_blocking(parent_paths: &ThreadPaths) -> Vec<ChildIndexEntry> {
    fold_child_runs(read_child_runs_blocking(parent_paths))
}

/// Rewrite `children/index.json` from the run log.
pub fn rebuild_child_index_blocking(parent_paths: &ThreadPaths) -> Result<ChildIndexFile> {
    let index = index_from_runs(read_child_runs_blocking(parent_paths));
    ensure_dir(&parent_paths.children_dir())?;
    write_json_atomic(&parent_paths.children_index(), &index)?;
    Ok(index)
}

/// Read `children/index.json` (cache only; never used for authorization).
pub fn read_child_index_blocking(parent_paths: &ThreadPaths) -> Option<ChildIndexFile> {
    read_json_opt(&parent_paths.children_index())
}

/// Drop every record for `child` from `children/runs.jsonl`.
///
/// The run log is the authority, so destroying a child means rewriting it
/// without that child's lines. The rewrite is temp-file + rename, never an
/// in-place truncation: a crash mid-delete must leave either the old log or the
/// new one, and a partly-written log would make surviving children disappear.
///
/// Returns `false` when the child had no records at all.
pub fn remove_child_run_blocking(
    parent_paths: &ThreadPaths,
    child: ChildId,
    fsync: bool,
) -> Result<bool> {
    let records = read_child_runs_blocking(parent_paths);
    if !records.iter().any(|record| record.child_id == child) {
        return Ok(false);
    }
    let mut kept = String::new();
    for record in records.iter().filter(|record| record.child_id != child) {
        kept.push_str(&encode_line(record)?);
    }
    // An empty rewrite still has to create the file: the parent keeps its
    // `children/runs.jsonl` so the next append has somewhere to go.
    ensure_dir(&parent_paths.children_dir())?;
    crate::store::fs_util::write_atomic(&parent_paths.children_runs(), kept.as_bytes())?;
    let _ = fsync;
    rebuild_child_index_blocking(parent_paths)?;
    Ok(true)
}

/// Delete one child's session directory, if it exists.
pub fn remove_child_dir_blocking(parent_paths: &ThreadPaths, child: ChildId) -> Result<()> {
    let dir = parent_paths.children_dir().join(child.to_string());
    if !dir.exists() {
        return Ok(());
    }
    remove_dir_all(&dir)
}

/// Whether `path_name` is one of the transient "being deleted" directories.
pub fn is_deleting_dir(name: &str) -> bool {
    name.starts_with(names::DELETING_PREFIX)
}

/// Rebuild the cache and emit `index_repaired` when it disagreed with the log.
///
/// Returns the authoritative entries regardless of whether a repair was needed.
pub fn sync_child_index_blocking(
    parent_paths: &ThreadPaths,
    parent: ThreadId,
    diagnostics: &dyn DiagnosticSink,
) -> Result<Vec<ChildIndexEntry>> {
    let records = read_child_runs_blocking(parent_paths);
    let expected = index_from_runs(records);
    let on_disk = read_child_index_blocking(parent_paths);
    let needs_write = on_disk.as_ref() != Some(&expected);
    if needs_write {
        ensure_dir(&parent_paths.children_dir())?;
        write_json_atomic(&parent_paths.children_index(), &expected)?;
        if on_disk.is_some() {
            diagnostics.record(
                StoreDiagnostic::new(
                    DiagnosticKind::IndexRepaired,
                    format!(
                        "children/index.json rebuilt from runs.jsonl ({} entries)",
                        expected.children.len()
                    ),
                )
                .for_thread(parent),
            );
        }
    }
    Ok(expected.children)
}

/// Scan one parent directory and absorb its children into `locator`.
///
/// Returns the ids whose location disagreed with the previously known one, which
/// the caller reports as `index_repaired` (design 7.4.1).
pub fn absorb_parent_blocking(
    locator: &mut ChildLocator,
    parent: ThreadId,
    parent_paths: &ThreadPaths,
) -> Vec<ChildId> {
    let records = read_child_runs_blocking(parent_paths);
    locator.absorb(parent, &records, |child| {
        format!(
            "{}/{}",
            names::THREADS,
            format_args!("{parent}/children/{child}")
        )
    })
}

/// Directory-only children that have no authoritative record (orphans).
///
/// Used by the startup cleanup: a child directory without a run record is data
/// loss waiting to happen, so it is reported rather than deleted.
pub fn orphan_child_dirs_blocking(parent_paths: &ThreadPaths) -> Vec<String> {
    let known: Vec<ChildId> = read_child_runs_blocking(parent_paths)
        .into_iter()
        .map(|record| record.child_id)
        .collect();
    list_dirs(&parent_paths.children_dir())
        .into_iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !is_deleting_dir(name))
        .filter(|name| !known.iter().any(|child| child.to_string() == *name))
        .collect()
}

/// Rename `children/` to `children.deleting-<uuid>` and return the new path.
///
/// Renaming first (instead of deleting in place) means a crash mid-delete leaves
/// a directory the next startup can recognize and clean up, instead of a parent
/// whose `children/` is half gone (design 7.4.1).
pub fn begin_children_delete_blocking(parent_paths: &ThreadPaths) -> Result<ThreadPaths> {
    let children = parent_paths.children_dir();
    if !children.exists() {
        return Ok(ThreadPaths::new(children));
    }
    let tombstone = parent_paths.dir().join(format!(
        "{}{}",
        names::DELETING_PREFIX,
        uuid::Uuid::new_v4()
    ));
    rename_dir(&children, &tombstone)?;
    // Re-create an empty children dir so new children can still be recorded.
    ensure_dir(&children)?;
    ensure_dir(&tombstone)?;
    Ok(ThreadPaths::new(tombstone))
}

/// Delete a tombstone directory (called after the writers have been closed).
pub fn finish_children_delete_blocking(tombstone: &ThreadPaths) -> Result<()> {
    if is_deleting_dir(
        tombstone
            .dir()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .as_str(),
    ) {
        return remove_dir_all(tombstone.dir());
    }
    Err(ArkError::store(format!(
        "refusing to delete {}: not a tombstone directory",
        tombstone.dir().display()
    )))
}

/// Tombstone directories left behind by a crash; startup deletes these.
pub fn pending_tombstones_blocking(parent_paths: &ThreadPaths) -> Vec<ThreadPaths> {
    list_dirs(parent_paths.dir())
        .into_iter()
        .filter(|path| {
            path.file_name()
                .map(|name| is_deleting_dir(&name.to_string_lossy()))
                .unwrap_or(false)
        })
        .map(ThreadPaths::new)
        .collect()
}

/// Whether a parent still has an active child run in the authoritative log.
pub fn active_child_runs_blocking(parent_paths: &ThreadPaths) -> Vec<ChildIndexEntry> {
    child_entries_blocking(parent_paths)
        .into_iter()
        .filter(ChildIndexEntry::is_active)
        .collect()
}

/// Count the child run records that exist for `parent`.
pub fn child_count_blocking(parent_paths: &ThreadPaths) -> usize {
    read_child_runs_blocking(parent_paths).len()
}

/// Whether a child directory carries a `thread.json` (i.e. is a usable thread).
pub fn child_dir_ready_blocking(child_paths: &ThreadPaths) -> bool {
    is_file(&child_paths.thread_json())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::diagnostics::MemoryDiagnostics;
    use crate::store::fs_util::ensure_dir;
    use crate::store::tests::TempDir;
    use ark_common::TurnId;
    use chrono::Utc;

    fn record(child: ChildId, parent: ThreadId, status: &str) -> ChildRunRecord {
        let mut record = ChildRunRecord::queued(child, parent, TurnId::new(), "scan", Utc::now());
        record.status = status.into();
        record
    }

    fn parent_paths(dir: &TempDir) -> ThreadPaths {
        let paths = ThreadPaths::new(dir.path().join("parent"));
        ensure_dir(paths.dir()).unwrap();
        paths
    }

    #[test]
    fn runs_jsonl_is_authoritative_and_index_is_derived() {
        let dir = TempDir::new("child-authority");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        let child = ChildId::new();
        append_child_run_blocking(&paths, &record(child, parent, "running"), true).unwrap();
        append_child_run_blocking(&paths, &record(child, parent, "completed"), true).unwrap();

        // Corrupt the cache deliberately: the log still wins.
        std::fs::write(paths.children_index(), b"{\"children\":[]}").unwrap();
        let sink = MemoryDiagnostics::new();
        let entries = sync_child_index_blocking(&paths, parent, &sink).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "completed");
        assert!(sink.has(DiagnosticKind::IndexRepaired));
        let cache = read_child_index_blocking(&paths).unwrap();
        assert_eq!(cache.children.len(), 1);
    }

    #[test]
    fn a_missing_cache_is_written_without_a_repair_diagnostic() {
        let dir = TempDir::new("child-no-cache");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        append_child_run_blocking(&paths, &record(ChildId::new(), parent, "running"), true)
            .unwrap();
        let sink = MemoryDiagnostics::new();
        sync_child_index_blocking(&paths, parent, &sink).unwrap();
        assert!(!sink.has(DiagnosticKind::IndexRepaired));
        assert!(read_child_index_blocking(&paths).is_some());
    }

    #[test]
    fn active_runs_are_reported_for_the_delete_gate() {
        let dir = TempDir::new("child-active");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        append_child_run_blocking(&paths, &record(ChildId::new(), parent, "running"), false)
            .unwrap();
        let done = ChildId::new();
        append_child_run_blocking(&paths, &record(done, parent, "completed"), false).unwrap();
        let active = active_child_runs_blocking(&paths);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, "running");
        assert_eq!(child_count_blocking(&paths), 2);
    }

    #[test]
    fn deleting_a_parent_renames_then_removes_the_children_dir() {
        let dir = TempDir::new("child-tombstone");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        append_child_run_blocking(&paths, &record(ChildId::new(), parent, "completed"), false)
            .unwrap();

        let tombstone = begin_children_delete_blocking(&paths).unwrap();
        assert!(is_deleting_dir(
            tombstone
                .dir()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        ));
        assert!(
            paths.children_dir().is_dir(),
            "children/ is recreated empty"
        );
        assert!(tombstone.dir().join(names::CHILDREN_RUNS).is_file());
        assert_eq!(pending_tombstones_blocking(&paths).len(), 1);

        finish_children_delete_blocking(&tombstone).unwrap();
        assert!(!tombstone.dir().exists());
        assert!(pending_tombstones_blocking(&paths).is_empty());
    }

    #[test]
    fn finish_refuses_a_non_tombstone_path() {
        let dir = TempDir::new("child-refuse");
        let paths = parent_paths(&dir);
        ensure_dir(&paths.children_dir()).unwrap();
        let err = finish_children_delete_blocking(&paths).unwrap_err();
        assert!(err.message.contains("refusing"), "{}", err.message);
    }

    #[test]
    fn orphan_child_dirs_are_detected_not_deleted() {
        let dir = TempDir::new("child-orphan");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        let known = ChildId::new();
        append_child_run_blocking(&paths, &record(known, parent, "completed"), false).unwrap();
        ensure_dir(&paths.children_dir().join(known.to_string())).unwrap();
        let orphan = ChildId::new();
        ensure_dir(&paths.children_dir().join(orphan.to_string())).unwrap();

        let orphans = orphan_child_dirs_blocking(&paths);
        assert_eq!(orphans, vec![orphan.to_string()]);
        assert!(paths.children_dir().join(orphan.to_string()).is_dir());
    }

    #[test]
    fn locator_absorbs_records_per_parent() {
        let dir = TempDir::new("child-locator");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        let child = ChildId::new();
        append_child_run_blocking(&paths, &record(child, parent, "running"), false).unwrap();

        let mut locator = ChildLocator::default();
        assert!(absorb_parent_blocking(&mut locator, parent, &paths).is_empty());
        let location = locator.get(&child).expect("child is registered");
        assert_eq!(location.parent_thread_id, parent);
        assert!(location
            .rel_path
            .ends_with(&format!("{parent}/children/{child}")));
    }

    #[test]
    fn damaged_run_lines_are_skipped() {
        let dir = TempDir::new("child-damaged");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        let child = ChildId::new();
        append_child_run_blocking(&paths, &record(child, parent, "running"), false).unwrap();
        let mut raw = std::fs::read_to_string(paths.children_runs()).unwrap();
        raw.push_str("{\"child_id\": \n");
        std::fs::write(paths.children_runs(), raw).unwrap();
        assert_eq!(read_child_runs_blocking(&paths).len(), 1);
    }

    #[test]
    fn a_torn_tail_is_cut_before_the_next_record_is_appended() {
        // A crash between the payload write and the newline leaves a partial
        // JSON line *with no terminator*. Appending onto it would glue the new
        // record to the damaged bytes, so both lines become unparseable and the
        // queued/running child they describe silently disappears (P1-2).
        let dir = TempDir::new("child-torn-tail");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        let first = ChildId::new();
        append_child_run_blocking(&paths, &record(first, parent, "running"), false).unwrap();

        let torn = "{\"child_id\": ";
        let mut raw = std::fs::read_to_string(paths.children_runs()).unwrap();
        raw.push_str(torn);
        std::fs::write(paths.children_runs(), raw).unwrap();

        let second = ChildId::new();
        append_child_run_blocking(&paths, &record(second, parent, "queued"), false).unwrap();

        let repaired = std::fs::read_to_string(paths.children_runs()).unwrap();
        assert!(
            !repaired.contains(torn),
            "the torn tail must be gone: {repaired}"
        );
        let records = read_child_runs_blocking(&paths);
        assert_eq!(records.len(), 2, "both whole records must survive");
        assert_eq!(records[0].child_id, first);
        assert_eq!(records[1].child_id, second);
        assert_eq!(records[1].status, "queued");
    }

    #[test]
    fn terminated_but_invalid_lines_are_kept_as_mid_file_damage() {
        // A byte-for-byte invalid line that *is* newline-terminated cannot corrupt
        // the next record, so it must stay on disk: silently truncating it would
        // hide real corruption (design §7.4.1), and `read` already skips it.
        let dir = TempDir::new("child-terminated-garbage");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        append_child_run_blocking(&paths, &record(ChildId::new(), parent, "running"), false)
            .unwrap();
        let mut raw = std::fs::read_to_string(paths.children_runs()).unwrap();
        raw.push_str("{\"child_id\": \n");
        std::fs::write(paths.children_runs(), raw).unwrap();
        let before = std::fs::read_to_string(paths.children_runs()).unwrap();

        let removed = repair_child_runs_tail_blocking(&paths).unwrap();

        assert_eq!(removed, 0, "terminated damage is not a torn tail");
        assert_eq!(
            std::fs::read_to_string(paths.children_runs()).unwrap(),
            before
        );
    }

    #[test]
    fn a_clean_log_is_never_truncated() {
        let dir = TempDir::new("child-clean-tail");
        let paths = parent_paths(&dir);
        let parent = ThreadId::new();
        let child = ChildId::new();
        append_child_run_blocking(&paths, &record(child, parent, "queued"), false).unwrap();
        let before = std::fs::read_to_string(paths.children_runs()).unwrap();

        let removed = repair_child_runs_tail_blocking(&paths).unwrap();

        assert_eq!(removed, 0, "a clean log has nothing to repair");
        assert_eq!(
            std::fs::read_to_string(paths.children_runs()).unwrap(),
            before,
            "repair must not rewrite bytes it did not need to touch"
        );
    }
}
