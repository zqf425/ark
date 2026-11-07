//! Crash-safe compaction transaction for `events.jsonl` and its sidecar.
//!
//! A pending line is durable before the event log is replaced, but is not a
//! committed hole by itself. Readers expose its marker only when the current
//! event bytes match the planned compacted body. Recovery either removes the
//! intention (old bytes) or commits it (new bytes).

use crate::domain::compaction::{parse_marker, CompactionMarker};
use crate::domain::layout::ThreadPaths;
use crate::store::fs_util::{path_entry_exists, sync_parent_dir, write_atomic};
use ark_common::{ArkError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::ErrorKind;

const PENDING_KIND: &str = "compaction_pending";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingCompaction {
    kind: String,
    transaction_id: uuid::Uuid,
    marker: CompactionMarker,
    before_sha256: String,
    after_sha256: String,
}

pub(crate) struct CompactionTransaction {
    paths: ThreadPaths,
}

impl CompactionTransaction {
    /// Persist an intention before the authoritative event log can change.
    pub(crate) fn begin(
        paths: &ThreadPaths,
        marker: CompactionMarker,
        after: &[u8],
    ) -> Result<Self> {
        recover(paths)?;
        let mut sidecar = read_sidecar(paths)?;
        validate_existing_sidecar(paths, &sidecar)?;
        ensure_trailing_newline(&mut sidecar);
        let pending = PendingCompaction {
            kind: PENDING_KIND.to_string(),
            transaction_id: uuid::Uuid::new_v4(),
            marker,
            before_sha256: digest(&read_events(paths)?),
            after_sha256: digest(after),
        };
        sidecar.extend_from_slice(serde_json::to_string(&pending)?.as_bytes());
        sidecar.push(b'\n');
        persist_sidecar(paths, &sidecar)?;
        Ok(Self {
            paths: paths.clone(),
        })
    }

    /// Commit the pending marker after the compacted event bytes are durable.
    pub(crate) fn commit(self) -> Result<()> {
        recover(&self.paths)
    }
}

/// Resolve a prior interrupted transaction from the current event-byte hash.
pub(crate) fn recover(paths: &ThreadPaths) -> Result<()> {
    let sidecar = read_sidecar(paths)?;
    if !contains_pending(&sidecar) {
        return Ok(());
    }
    let events = read_events(paths)?;
    let current = digest(&events);
    let mut changed = false;
    let mut resolved = Vec::with_capacity(sidecar.len());
    for line in sidecar.split_inclusive(|byte| *byte == b'\n') {
        if let Some(pending) = parse_pending(line) {
            changed = true;
            resolve_line(&mut resolved, &pending, &current)?;
        } else {
            resolved.extend_from_slice(line);
        }
    }
    if changed {
        persist_or_remove_sidecar(paths, &resolved)?;
    }
    Ok(())
}

/// Read committed markers plus a pending marker whose compacted bytes exist.
pub(crate) fn effective_markers(paths: &ThreadPaths) -> Vec<CompactionMarker> {
    let Ok(sidecar) = read_sidecar(paths) else {
        return Vec::new();
    };
    let current = read_events(paths).ok().map(|bytes| digest(&bytes));
    sidecar
        .split(|byte| *byte == b'\n')
        .filter_map(|line| effective_marker(line, current.as_deref()))
        .collect()
}

/// Whether the sidecar proves history is absent or unsafe to treat as complete.
///
/// A valid pending intention over unchanged event bytes is the sole existing
/// sidecar shape that does not declare a hole. Empty, malformed, unreadable, or
/// hash-mismatched sidecars remain conservative incomplete-history signals.
pub(crate) fn has_effective_history(paths: &ThreadPaths) -> bool {
    if !path_entry_exists(&paths.compacted_jsonl()) {
        return false;
    }
    let Ok(sidecar) = read_sidecar(paths) else {
        return true;
    };
    let Ok(current) = read_events(paths).map(|bytes| digest(&bytes)) else {
        return true;
    };
    let mut saw_line = false;
    for line in sidecar.split(|byte| *byte == b'\n') {
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }
        saw_line = true;
        if parse_marker(std::str::from_utf8(line).unwrap_or_default()).is_some() {
            return true;
        }
        match parse_pending(line) {
            Some(pending) if pending.before_sha256 == current => {}
            _ => return true,
        }
    }
    !saw_line
}

fn effective_marker(line: &[u8], current: Option<&str>) -> Option<CompactionMarker> {
    let text = std::str::from_utf8(line).ok()?;
    if let Some(marker) = parse_marker(text) {
        return Some(marker);
    }
    let pending = parse_pending(line)?;
    (current == Some(pending.after_sha256.as_str())).then_some(pending.marker)
}

fn resolve_line(out: &mut Vec<u8>, pending: &PendingCompaction, current: &str) -> Result<()> {
    if current == pending.before_sha256 {
        return Ok(());
    }
    if current == pending.after_sha256 {
        out.extend_from_slice(pending.marker.to_line().as_bytes());
        out.push(b'\n');
        return Ok(());
    }
    Err(ArkError::store(format!(
        "cannot recover compaction transaction {}: events.jsonl matches neither old nor compacted bytes",
        pending.transaction_id
    )))
}

fn parse_pending(line: &[u8]) -> Option<PendingCompaction> {
    let pending = serde_json::from_slice::<PendingCompaction>(line).ok()?;
    (pending.kind == PENDING_KIND).then_some(pending)
}

fn contains_pending(sidecar: &[u8]) -> bool {
    sidecar
        .split(|byte| *byte == b'\n')
        .any(|line| parse_pending(line).is_some())
}

fn read_sidecar(paths: &ThreadPaths) -> Result<Vec<u8>> {
    read_optional(&paths.compacted_jsonl(), "compaction sidecar")
}

fn validate_existing_sidecar(paths: &ThreadPaths, bytes: &[u8]) -> Result<()> {
    if !path_entry_exists(&paths.compacted_jsonl()) {
        return Ok(());
    }
    let valid = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| line.iter().any(|byte| !byte.is_ascii_whitespace()))
        .all(|line| {
            std::str::from_utf8(line)
                .ok()
                .and_then(parse_marker)
                .is_some()
        });
    if valid && bytes.iter().any(|byte| !byte.is_ascii_whitespace()) {
        return Ok(());
    }
    Err(ArkError::store(format!(
        "refusing compaction with invalid existing sidecar {}",
        paths.compacted_jsonl().display()
    )))
}

fn read_events(paths: &ThreadPaths) -> Result<Vec<u8>> {
    read_optional(&paths.events_jsonl(), "event log")
}

fn read_optional(path: &std::path::Path, label: &str) -> Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(ArkError::store(format!(
            "read {label} failed for {}: {error}",
            path.display()
        ))),
    }
}

fn persist_sidecar(paths: &ThreadPaths, bytes: &[u8]) -> Result<()> {
    let path = paths.compacted_jsonl();
    write_atomic(&path, bytes)?;
    sync_parent_dir(&path)
}

fn persist_or_remove_sidecar(paths: &ThreadPaths, bytes: &[u8]) -> Result<()> {
    if bytes.iter().any(|byte| !byte.is_ascii_whitespace()) {
        return persist_sidecar(paths, bytes);
    }
    let path = paths.compacted_jsonl();
    if path_entry_exists(&path) {
        std::fs::remove_file(&path).map_err(|error| {
            ArkError::store(format!("remove compaction intention failed: {error}"))
        })?;
        sync_parent_dir(&path)?;
    }
    Ok(())
}

fn ensure_trailing_newline(bytes: &mut Vec<u8>) {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
}

fn digest(bytes: &[u8]) -> String {
    let value = Sha256::digest(bytes);
    let mut output = String::with_capacity(value.len() * 2);
    for byte in value {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::event_log::{encode_event, plan_compaction};
    use crate::store::tests::{event_at, TempDir};
    use ark_common::ThreadId;

    fn fixture(tag: &str) -> (TempDir, ThreadPaths, Vec<u8>, Vec<u8>, CompactionMarker) {
        let dir = TempDir::new(tag);
        let paths = ThreadPaths::new(dir.path().join("thread"));
        crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
        let thread = ThreadId::new();
        let events = (1..=3).map(|seq| event_at(thread, seq)).collect::<Vec<_>>();
        let before = events
            .iter()
            .map(|event| encode_event(event).unwrap())
            .collect::<String>()
            .into_bytes();
        let (after, marker) = plan_compaction(&events, 1, "test").unwrap().unwrap();
        std::fs::write(paths.events_jsonl(), &before).unwrap();
        (dir, paths, before, after.into_bytes(), marker)
    }

    #[test]
    fn pending_intention_with_old_events_is_not_a_hole_and_is_removed() {
        let (_dir, paths, before, after, marker) = fixture("compact-pending-old");
        let _transaction = CompactionTransaction::begin(&paths, marker, &after).unwrap();

        assert!(effective_markers(&paths).is_empty());
        assert!(!has_effective_history(&paths));
        recover(&paths).unwrap();

        assert_eq!(std::fs::read(paths.events_jsonl()).unwrap(), before);
        assert!(!path_entry_exists(&paths.compacted_jsonl()));
    }

    #[test]
    fn pending_intention_with_new_events_is_a_hole_and_is_committed() {
        let (_dir, paths, _before, after, marker) = fixture("compact-pending-new");
        let _transaction = CompactionTransaction::begin(&paths, marker.clone(), &after).unwrap();
        write_atomic(&paths.events_jsonl(), &after).unwrap();

        assert_eq!(effective_markers(&paths), vec![marker.clone()]);
        assert!(has_effective_history(&paths));
        recover(&paths).unwrap();

        assert_eq!(effective_markers(&paths), vec![marker]);
        let sidecar = std::fs::read_to_string(paths.compacted_jsonl()).unwrap();
        assert!(!sidecar.contains(PENDING_KIND));
    }
}
