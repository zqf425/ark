//! `items.jsonl` and `usage.jsonl`: derived logs, rebuildable from events.
//!
//! Design 7.4.1. Both files are projections, so a read never trusts them
//! blindly: a missing or empty file is rebuilt from `events.jsonl`, and a
//! damaged line is skipped rather than failing the load.

use crate::domain::items::{delta_item_id, fold_item_snapshots, project_items};
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

/// One decoded `items.jsonl` row.
///
/// `Watermarked` is the current format: an item snapshot plus the event
/// interval the row already covers. `Bare` is a pre-watermark row holding only
/// the item. The distinction decides whether the file can be read
/// incrementally or must be rebuilt from the whole event log.
#[derive(Debug, Clone)]
pub enum ItemRow {
    Watermarked(crate::domain::items::ItemSnapshotRecord),
    Bare(ThreadItem),
}

impl ItemRow {
    fn is_legacy(&self) -> bool {
        match self {
            ItemRow::Watermarked(record) => record.is_legacy_shape(),
            ItemRow::Bare(_) => true,
        }
    }
}

/// Append one watermarked item snapshot to `items.jsonl`.
pub fn append_item_record_blocking(
    paths: &ThreadPaths,
    record: &crate::domain::items::ItemSnapshotRecord,
    fsync: bool,
) -> Result<()> {
    append_text(&paths.items_jsonl(), &encode_line(record)?, fsync)
}

/// Rewrite `items.jsonl` from watermarked snapshot rows (atomic temp file).
pub fn write_items_records_blocking(
    paths: &ThreadPaths,
    records: &[crate::domain::items::ItemSnapshotRecord],
) -> Result<()> {
    let mut body = String::new();
    for record in records {
        body.push_str(&encode_line(record)?);
    }
    crate::store::fs_util::write_atomic(&paths.items_jsonl(), body.as_bytes())
}

/// Rewrite `items.jsonl` from the authoritative event log and return the items.
///
/// The rewrite emits watermarked rows, so a legacy or stale file is upgraded in
/// place instead of forcing every later read through a full-log replay. Used by
/// the rebuild paths (fork, rewind, reconcile) which already hold the events.
pub fn rewrite_items_from_events_blocking(
    paths: &ThreadPaths,
    events: &[RuntimeEvent],
) -> Result<Vec<ThreadItem>> {
    let mut projector = crate::domain::items::ItemProjector::new();
    for event in events {
        projector.apply(event);
    }
    let records = projector.snapshot_records();
    write_items_records_blocking(paths, &records)?;
    Ok(records.into_iter().map(|record| record.item).collect())
}

/// Read every parsable row (damaged lines are skipped).
///
/// Each line is first tried as a watermarked record and then as a bare
/// pre-watermark item, so an old file still decodes instead of failing.
pub fn read_item_rows_blocking(paths: &ThreadPaths) -> Vec<ItemRow> {
    let raw = match ark_common::read_text_opt(&paths.items_jsonl()) {
        Some(raw) => raw,
        None => return Vec::new(),
    };
    raw.lines()
        .filter_map(|line| {
            serde_json::from_str::<crate::domain::items::ItemSnapshotRecord>(line)
                .map(ItemRow::Watermarked)
                .or_else(|_| serde_json::from_str::<ThreadItem>(line).map(ItemRow::Bare))
                .ok()
        })
        .collect()
}

/// Rebuild `items.jsonl` from the event log and return the rebuilt stream.
pub fn rebuild_items_blocking(paths: &ThreadPaths) -> Result<Vec<ThreadItem>> {
    let events = read_log_blocking(paths)?.events;
    rewrite_items_from_events_blocking(paths, &events)
}

/// Items for the thread: prefer `items.jsonl`, fall back to a projection.
///
/// One legacy row (a bare item or a zero watermark) means the whole file was
/// written before watermarking; its rows already contain every delta, so a
/// replay on top of them would double-count text. With a non-empty event log
/// the log is authoritative and the file is ignored entirely. When the log
/// itself is empty (for example a damaged log next to a recoverable items
/// file) the surviving rows are folded instead, so recovery keeps returning
/// the items the projection used to see. A pure watermarked file is read
/// incrementally: rows seed the projection, then only delta events past each
/// row's watermark are replayed. The watermark is per item, not one global
/// cutoff: an item that never earned a row may have its first delta before
/// another item's watermark, and a global cutoff would drop it. Non-delta
/// events are never replayed — the writer never skipped them, so their effect
/// is already inside the rows.
pub fn read_items_blocking(paths: &ThreadPaths) -> Result<Vec<ThreadItem>> {
    let rows = read_item_rows_blocking(paths);
    if rows.is_empty() || rows.iter().any(ItemRow::is_legacy) {
        let events = read_log_blocking(paths)?.events;
        if events.is_empty() {
            // Damaged-log recovery: `items.jsonl` is the only surviving source.
            return Ok(fold_item_snapshots(rows.into_iter().map(|row| match row {
                ItemRow::Watermarked(record) => record.item,
                ItemRow::Bare(item) => item,
            })));
        }
        return Ok(project_items(&events));
    }
    let events = read_log_blocking(paths)?.events;
    let mut projector = crate::domain::items::ItemProjector::new();
    for row in rows {
        match row {
            ItemRow::Watermarked(record) => {
                projector.seed_snapshot(record.first_seq, record.watermark_seq, record.item)
            }
            // The legacy branch above already returned for bare rows.
            ItemRow::Bare(_) => {}
        }
    }
    for event in &events {
        if let Some(item_id) = delta_item_id(event) {
            if event.seq() > projector.watermark_of(&item_id) {
                projector.apply(event);
            }
        }
    }
    Ok(projector.finish_in_event_order())
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

        let rows = read_item_rows_blocking(&paths);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].is_legacy(), "a bare row is the legacy shape");
        assert_eq!(read_items_blocking(&paths).unwrap().len(), 1);
    }

    /// Append `item` as a bare pre-watermark row (the old `items.jsonl` shape).
    fn append_legacy_row(paths: &ThreadPaths, item: &ThreadItem) {
        let existing = ark_common::read_text_opt(&paths.items_jsonl()).unwrap_or_default();
        std::fs::write(
            paths.items_jsonl(),
            format!("{}{}", existing, encode_line(item).unwrap()),
        )
        .unwrap();
    }

    #[test]
    fn a_legacy_bare_row_file_replays_the_whole_log() {
        // A pre-watermark row already contains every delta, so replaying the
        // log on top of it would double the text. The read must therefore
        // ignore the file entirely and project from the authoritative events.
        let dir = TempDir::new("items-legacy");
        let paths = paths(&dir);
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let item = ItemId::new();
        let events = vec![
            turn_started(thread, turn, 1),
            text_delta(thread, turn, item, 2, "a"),
            text_delta(thread, turn, item, 3, "b"),
        ];
        crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
        append_legacy_row(&paths, &project_items(&events)[0]);
        std::fs::write(
            paths.events_jsonl(),
            events
                .iter()
                .map(|event| encode_line(event).unwrap())
                .collect::<String>(),
        )
        .unwrap();

        let items = read_items_blocking(&paths).unwrap();
        assert_eq!(items, project_items(&events));
        assert_eq!(items[0].text.as_deref(), Some("ab"));
    }

    #[test]
    fn watermarked_rows_read_back_equal_a_full_replay() {
        // Covers the three shapes that decide correctness of the incremental
        // read: a delta-only item (no row at all), a delta-then-keypoint item
        // (row plus later delta replay) and multiple interleaved items.
        let dir = TempDir::new("items-watermarked");
        let paths = paths(&dir);
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let text_item = ItemId::new();
        let tool_call = "call-1";
        let events = vec![
            turn_started(thread, turn, 1),
            // Delta-only so far: no row may exist for it yet.
            text_delta(thread, turn, text_item, 2, "he"),
            RuntimeEvent::ToolCallStarted {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                timestamp: Utc::now(),
                call_id: tool_call.into(),
                tool: "bash".into(),
                arguments: serde_json::json!({}),
            },
            RuntimeEvent::ToolCallFinished {
                seq: 4,
                thread_id: thread,
                turn_id: turn,
                timestamp: Utc::now(),
                call_id: tool_call.into(),
                tool: "bash".into(),
                ok: true,
                duration_ms: 12,
                output: "ok".into(),
                detail: None,
            },
            // A late delta after the item's row: replay must append exactly it.
            text_delta(thread, turn, text_item, 5, "llo"),
            // A second delta-only item created after other items' watermarks.
            text_delta(thread, turn, ItemId::new(), 6, "second"),
        ];
        crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
        std::fs::write(
            paths.events_jsonl(),
            events
                .iter()
                .map(|event| encode_line(event).unwrap())
                .collect::<String>(),
        )
        .unwrap();

        // Mirror the writer policy: only non-delta item events append rows.
        let mut projector = crate::domain::items::ItemProjector::new();
        for event in &events {
            if let Some(item) = projector.apply(event) {
                if !crate::domain::items::is_delta_event(event) {
                    append_item_record_blocking(
                        &paths,
                        &crate::domain::items::ItemSnapshotRecord::from_projection(
                            projector.first_seq_of(&item.item_id),
                            projector.watermark_of(&item.item_id),
                            &item,
                        ),
                        false,
                    )
                    .unwrap();
                }
            }
        }

        let items = read_items_blocking(&paths).unwrap();
        assert_eq!(items, project_items(&events));
        assert_eq!(items[0].text.as_deref(), Some("hello"));
    }

    #[test]
    fn usage_increments_fold_and_rebuild_from_events() {
        let dir = TempDir::new("usage-fold");
        let paths = paths(&dir);
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
