//! `items.jsonl` and `usage.jsonl`: derived logs, rebuildable from events.
//!
//! Design 7.4.1. Both files are projections, so a read never trusts them
//! blindly: a missing or empty file is rebuilt from `events.jsonl`, and a
//! damaged line is skipped rather than failing the load.

use crate::domain::items::{fold_item_snapshots, project_items};
use crate::domain::layout::ThreadPaths;
use crate::store::event_log::{append_text, encode_line, read_log_blocking};
use ark_common::{Result, TurnId, Usage};
use ark_contract::{RuntimeEvent, ThreadItem};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One appended usage increment (design 6.4: usage is additive).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub usage: Usage,
    pub at: DateTime<Utc>,
}

impl UsageRecord {
    pub fn from_event(seq: u64, turn_id: TurnId, usage: Usage) -> Self {
        Self {
            seq,
            turn_id: Some(turn_id),
            usage,
            at: Utc::now(),
        }
    }
}

/// Append one item snapshot to `items.jsonl`.
pub fn append_item_blocking(paths: &ThreadPaths, item: &ThreadItem, fsync: bool) -> Result<()> {
    append_text(&paths.items_jsonl(), &encode_line(item)?, fsync)
}

/// Rewrite `items.jsonl` from an authoritative item stream.
pub fn write_items_blocking(paths: &ThreadPaths, items: &[ThreadItem]) -> Result<()> {
    let mut body = String::new();
    for item in items {
        body.push_str(&encode_line(item)?);
    }
    crate::store::fs_util::write_atomic(&paths.items_jsonl(), body.as_bytes())
}

/// Read every parsable item snapshot (damaged lines are skipped).
///
/// Decoding goes through the shared policy so a BOM at the head of the file
/// cannot hide the first (or every) record from a strict JSON parser.
pub fn read_item_snapshots_blocking(paths: &ThreadPaths) -> Vec<ThreadItem> {
    let raw = match ark_common::read_text_opt(&paths.items_jsonl()) {
        Some(raw) => raw,
        None => return Vec::new(),
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<ThreadItem>(line).ok())
        .collect()
}

/// Rebuild `items.jsonl` from the event log and return the rebuilt stream.
pub fn rebuild_items_blocking(paths: &ThreadPaths) -> Result<Vec<ThreadItem>> {
    let events = read_log_blocking(paths)?.events;
    let items = project_items(&events);
    write_items_blocking(paths, &items)?;
    Ok(items)
}

/// Items for the thread: prefer `items.jsonl`, fall back to a projection.
pub fn read_items_blocking(paths: &ThreadPaths) -> Result<Vec<ThreadItem>> {
    let snapshots = read_item_snapshots_blocking(paths);
    if !snapshots.is_empty() {
        return Ok(fold_item_snapshots(snapshots));
    }
    let events = read_log_blocking(paths)?.events;
    Ok(project_items(&events))
}

/// Append one usage increment to `usage.jsonl`.
pub fn append_usage_blocking(paths: &ThreadPaths, record: &UsageRecord, fsync: bool) -> Result<()> {
    append_text(&paths.usage_jsonl(), &encode_line(record)?, fsync)
}

/// Read every parsable usage increment.
pub fn read_usage_blocking(paths: &ThreadPaths) -> Vec<UsageRecord> {
    let raw = match ark_common::read_text_opt(&paths.usage_jsonl()) {
        Some(raw) => raw,
        None => return Vec::new(),
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<UsageRecord>(line).ok())
        .collect()
}

/// Fold all usage increments into one total.
pub fn usage_totals(records: &[UsageRecord]) -> Usage {
    let mut total = Usage::default();
    for record in records {
        total.merge(&record.usage);
    }
    total
}

/// Usage totals for a thread; rebuilds from events when the log is absent.
pub fn thread_usage_blocking(paths: &ThreadPaths) -> Result<Usage> {
    let records = read_usage_blocking(paths);
    if !records.is_empty() {
        return Ok(usage_totals(&records));
    }
    let mut total = Usage::default();
    for event in read_log_blocking(paths)?.events {
        if let RuntimeEvent::Usage { usage, .. } = event {
            total.merge(&usage);
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::{text_delta, turn_started, TempDir};
    use ark_common::{ItemId, ThreadId};

    fn paths(dir: &TempDir) -> ThreadPaths {
        ThreadPaths::new(dir.path().join("thread"))
    }

    #[test]
    fn items_fall_back_to_a_projection_when_the_file_is_missing() {
        let dir = TempDir::new("items-fallback");
        let paths = paths(&dir);
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let item = ItemId::new();
        let mut body = String::new();
        body.push_str(
            &crate::store::event_log::encode_event(&turn_started(thread, turn, 1)).unwrap(),
        );
        body.push_str(&encode_line(&text_delta(thread, turn, item, 2, "hi")).unwrap());
        crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
        std::fs::write(paths.events_jsonl(), body).unwrap();

        let items = read_items_blocking(&paths).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text.as_deref(), Some("hi"));
    }

    #[test]
    fn rebuilt_items_file_equals_a_fresh_projection() {
        let dir = TempDir::new("items-rebuild");
        let paths = paths(&dir);
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let item = ItemId::new();
        let mut body = String::new();
        body.push_str(&encode_line(&turn_started(thread, turn, 1)).unwrap());
        body.push_str(&encode_line(&text_delta(thread, turn, item, 2, "a")).unwrap());
        body.push_str(&encode_line(&text_delta(thread, turn, item, 3, "b")).unwrap());
        crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
        std::fs::write(paths.events_jsonl(), body).unwrap();

        let rebuilt = rebuild_items_blocking(&paths).unwrap();
        let from_file = read_items_blocking(&paths).unwrap();
        assert_eq!(rebuilt, from_file);
        assert_eq!(from_file[0].text.as_deref(), Some("ab"));
    }

    #[test]
    fn damaged_item_line_is_skipped_not_fatal() {
        let dir = TempDir::new("items-damaged");
        let paths = paths(&dir);
        let item =
            ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::Notice)
                .with_text("ok");
        crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
        let mut body = String::new();
        body.push_str(&encode_line(&item).unwrap());
        body.push_str("{\"item_id\": broken\n");
        std::fs::write(paths.items_jsonl(), body).unwrap();

        let snapshots = read_item_snapshots_blocking(&paths);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(read_items_blocking(&paths).unwrap().len(), 1);
    }

    #[test]
    fn usage_increments_fold_and_rebuild_from_events() {
        let dir = TempDir::new("usage-fold");
        let paths = paths(&dir);
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let first = UsageRecord::from_event(1, turn, Usage::new(10, 5));
        let second = UsageRecord::from_event(2, turn, Usage::new(20, 5));
        append_usage_blocking(&paths, &first, false).unwrap();
        append_usage_blocking(&paths, &second, false).unwrap();

        let totals = thread_usage_blocking(&paths).unwrap();
        assert_eq!(totals.prompt_tokens, 30);
        assert_eq!(totals.completion_tokens, 10);
        assert_eq!(totals.total_tokens, 40);
    }

    #[test]
    fn usage_falls_back_to_usage_events() {
        let dir = TempDir::new("usage-events");
        let paths = paths(&dir);
        let thread = ThreadId::new();
        let event = RuntimeEvent::Usage {
            seq: 1,
            thread_id: thread,
            turn_id: TurnId::new(),
            timestamp: Utc::now(),
            usage: Usage::new(7, 3),
        };
        crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
        std::fs::write(
            paths.events_jsonl(),
            encode_line(&event).unwrap().as_bytes(),
        )
        .unwrap();
        assert_eq!(thread_usage_blocking(&paths).unwrap().total_tokens, 10);
    }
}
