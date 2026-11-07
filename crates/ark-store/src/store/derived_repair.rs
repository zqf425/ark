//! Validation and best-effort repair of event-derived thread files.
//!
//! Complete logs can validate a full projection. Dirty compacted logs instead
//! resume from the existing checkpoints and apply only the verified suffix.

use crate::domain::diagnostics::{DiagnosticKind, DiagnosticSink, StoreDiagnostic};
use crate::domain::items::{affects_items, ItemProjector, ItemSnapshotRecord};
use crate::domain::layout::ThreadPaths;
use crate::domain::session_state::SessionState;
use crate::store::event_log::encode_line;
use crate::store::fs_util::{read_json_opt, write_atomic, write_json_atomic};
use crate::store::items_log::{
    read_item_rows_checked_blocking, read_usage_checked_blocking, write_items_records_blocking,
    ItemRow, UsageRecord,
};
use ark_common::{ArkError, Result, ThreadId};
use ark_contract::RuntimeEvent;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Whether an event can change at least one on-disk derived projection.
pub(crate) fn affects_derived(event: &RuntimeEvent) -> bool {
    affects_items(event) || affects_usage(event) || affects_session(event)
}

/// Validate and repair all projections from a complete authoritative log.
pub(crate) fn repair_derived_from_events(
    paths: &ThreadPaths,
    thread_id: ThreadId,
    events: &[RuntimeEvent],
    diagnostics: &Arc<dyn DiagnosticSink>,
) -> bool {
    let items = repair_items_full(paths, thread_id, events, diagnostics);
    let usage = repair_usage_full(paths, thread_id, events, diagnostics);
    let session = repair_session_full(paths, thread_id, events, diagnostics);
    items && usage && session
}

/// Resume all applicable projections from checkpoints at `planned_seq`.
///
/// Each category is independently idempotent. A category untouched by the
/// suffix is successful without requiring its checkpoint to exist or parse.
pub(crate) fn repair_derived_from_checkpoint(
    paths: &ThreadPaths,
    thread_id: ThreadId,
    suffix: &[RuntimeEvent],
    pre_session: &SessionState,
    pre_current_turn: Option<ark_common::TurnId>,
    diagnostics: &Arc<dyn DiagnosticSink>,
) -> bool {
    let items = repair_items_checkpoint(paths, thread_id, suffix, pre_current_turn, diagnostics);
    let usage = repair_usage_checkpoint(paths, thread_id, suffix, diagnostics);
    let session = repair_session_checkpoint(paths, thread_id, suffix, pre_session, diagnostics);
    items && usage && session
}

fn repair_items_full(
    paths: &ThreadPaths,
    thread_id: ThreadId,
    events: &[RuntimeEvent],
    diagnostics: &Arc<dyn DiagnosticSink>,
) -> bool {
    let mut projector = ItemProjector::new();
    for event in events {
        projector.apply(event);
    }
    let expected = projector.snapshot_records();
    let (rows, all_valid) = read_item_rows_checked_blocking(paths);
    let actual = canonical_item_records(rows);
    if all_valid
        && actual
            .as_ref()
            .is_some_and(|records| same_item_records(records, &expected))
    {
        return true;
    }
    record_write(
        write_items_records_blocking(paths, &expected),
        "items.jsonl",
        thread_id,
        diagnostics,
        "complete events",
    )
}

fn repair_items_checkpoint(
    paths: &ThreadPaths,
    thread_id: ThreadId,
    suffix: &[RuntimeEvent],
    pre_current_turn: Option<ark_common::TurnId>,
    diagnostics: &Arc<dyn DiagnosticSink>,
) -> bool {
    if !suffix.iter().any(affects_items) {
        return true;
    }
    let (checkpoint, projector) = match load_item_checkpoint(paths, suffix, pre_current_turn) {
        Ok(loaded) => loaded,
        Err(_) => return record_invalid_checkpoint("items.jsonl", thread_id, diagnostics),
    };
    let expected = projector.snapshot_records();
    if same_item_records(&checkpoint, &expected) {
        return true;
    }
    record_write(
        write_items_records_blocking(paths, &expected),
        "items.jsonl",
        thread_id,
        diagnostics,
        "checkpoint and event suffix",
    )
}

/// Load the item and session checkpoints and replay the retained event suffix.
///
/// Writer startup uses the same parsers and watermark rules as startup repair,
/// so later incremental writes cannot replace a compacted prefix with a
/// suffix-only projection.
pub(crate) struct CheckpointRead<'a> {
    pub(crate) suffix: &'a [RuntimeEvent],
    pub(crate) current_turn: Option<ark_common::TurnId>,
}

/// Resolve the retained suffix and append-time turn context for a compacted read.
/// A dirty marker is evidence even when it cannot be parsed, so callers must
/// fail rather than fall back to a possibly advanced `session.json`.
pub(crate) fn checkpoint_read<'a>(
    paths: &ThreadPaths,
    events: &'a [RuntimeEvent],
) -> Result<CheckpointRead<'a>> {
    let recovery = crate::store::derived_dirty::recovery_plan(paths, events, false);
    if crate::store::derived_dirty::exists(paths) && recovery.is_none() {
        return Err(ArkError::store(
            "cannot read compacted dirty thread without valid pre-append context",
        ));
    }
    if let Some(plan) = recovery {
        let start = events
            .iter()
            .position(|event| event.seq() == plan.planned_seq)
            .ok_or_else(|| ArkError::store("dirty event suffix is missing"))?;
        return Ok(CheckpointRead {
            suffix: &events[start..],
            current_turn: plan.pre_current_turn,
        });
    }
    let current_turn = read_json_opt::<SessionState>(&paths.session_json())
        .and_then(|session| session.last_turn_id);
    Ok(CheckpointRead {
        suffix: events,
        current_turn,
    })
}

pub(crate) fn load_checkpoint_projections(
    paths: &ThreadPaths,
    suffix: &[RuntimeEvent],
    recovery: Option<&crate::store::derived_dirty::RecoveryPlan>,
) -> Result<(ItemProjector, SessionState)> {
    let mut session = match recovery {
        Some(plan) => plan.pre_session.clone(),
        None => load_session_checkpoint_base(paths)?,
    };
    let current_turn = recovery
        .map(|plan| plan.pre_current_turn)
        .unwrap_or(session.last_turn_id);
    let (_, projector) = load_item_checkpoint(paths, suffix, current_turn)?;
    for event in suffix {
        session.apply(event);
    }
    Ok((projector, session))
}

fn load_item_checkpoint(
    paths: &ThreadPaths,
    suffix: &[RuntimeEvent],
    current_turn: Option<ark_common::TurnId>,
) -> Result<(Vec<ItemSnapshotRecord>, ItemProjector)> {
    let (rows, all_valid) = read_item_rows_checked_blocking(paths);
    let checkpoint = all_valid
        .then(|| canonical_item_records(rows))
        .flatten()
        .ok_or_else(|| ark_common::ArkError::store("items checkpoint is missing or invalid"))?;
    let mut projector = ItemProjector::new();
    projector.seed_current_turn(current_turn);
    for record in &checkpoint {
        projector.seed_snapshot(record.first_seq, record.watermark_seq, record.item.clone());
    }
    for event in suffix {
        projector.apply_after_watermark(event);
    }
    Ok((checkpoint, projector))
}

fn canonical_item_records(rows: Vec<ItemRow>) -> Option<Vec<ItemSnapshotRecord>> {
    let mut latest = HashMap::new();
    for row in rows {
        let ItemRow::Watermarked(record) = row else {
            return None;
        };
        if record.is_legacy_shape() {
            return None;
        }
        latest.insert(record.item.item_id, record);
    }
    let mut records = latest.into_values().collect::<Vec<_>>();
    records.sort_by_key(|record| record.first_seq);
    Some(records)
}

fn same_item_records(left: &[ItemSnapshotRecord], right: &[ItemSnapshotRecord]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.first_seq == right.first_seq
                && left.watermark_seq == right.watermark_seq
                && left.item == right.item
        })
}

fn repair_usage_full(
    paths: &ThreadPaths,
    thread_id: ThreadId,
    events: &[RuntimeEvent],
    diagnostics: &Arc<dyn DiagnosticSink>,
) -> bool {
    let expected = usage_records(events);
    let (actual, all_valid) = read_usage_checked_blocking(paths);
    if all_valid && same_usage_records(&actual, &expected) {
        return true;
    }
    record_write(
        write_usage_records(paths, &expected),
        "usage.jsonl",
        thread_id,
        diagnostics,
        "complete events",
    )
}

fn repair_usage_checkpoint(
    paths: &ThreadPaths,
    thread_id: ThreadId,
    suffix: &[RuntimeEvent],
    diagnostics: &Arc<dyn DiagnosticSink>,
) -> bool {
    if !suffix.iter().any(affects_usage) {
        return true;
    }
    let (checkpoint, all_valid) = read_usage_checked_blocking(paths);
    if !all_valid {
        return record_invalid_checkpoint("usage.jsonl", thread_id, diagnostics);
    }
    let mut expected = Vec::with_capacity(checkpoint.len());
    let mut seen = HashSet::new();
    for record in &checkpoint {
        if seen.insert(record.seq) {
            expected.push(record.clone());
        }
    }
    for event_record in usage_records(suffix) {
        if let Some(existing) = expected
            .iter_mut()
            .find(|record| record.seq == event_record.seq)
        {
            *existing = event_record;
        } else {
            expected.push(event_record);
        }
    }
    expected.sort_by_key(|record| record.seq);
    if same_usage_records(&checkpoint, &expected) {
        return true;
    }
    record_write(
        write_usage_records(paths, &expected),
        "usage.jsonl",
        thread_id,
        diagnostics,
        "checkpoint and event suffix",
    )
}

fn affects_usage(event: &RuntimeEvent) -> bool {
    matches!(event, RuntimeEvent::Usage { .. })
}

fn usage_records(events: &[RuntimeEvent]) -> Vec<UsageRecord> {
    events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::Usage {
                seq,
                turn_id,
                timestamp,
                usage,
                ..
            } => Some(UsageRecord {
                seq: *seq,
                turn_id: Some(*turn_id),
                usage: usage.clone(),
                at: *timestamp,
            }),
            _ => None,
        })
        .collect()
}

fn same_usage_records(left: &[UsageRecord], right: &[UsageRecord]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.seq == right.seq && left.turn_id == right.turn_id && left.usage == right.usage
        })
}

fn write_usage_records(paths: &ThreadPaths, records: &[UsageRecord]) -> Result<()> {
    let mut body = String::new();
    for record in records {
        body.push_str(&encode_line(record)?);
    }
    write_atomic(&paths.usage_jsonl(), body.as_bytes())
}

fn repair_session_full(
    paths: &ThreadPaths,
    thread_id: ThreadId,
    events: &[RuntimeEvent],
    diagnostics: &Arc<dyn DiagnosticSink>,
) -> bool {
    let expected = SessionState::replay(events);
    let actual = read_json_opt::<SessionState>(&paths.session_json());
    if actual
        .as_ref()
        .is_some_and(|state| same_session(state, &expected))
    {
        return true;
    }
    record_write(
        write_json_atomic(&paths.session_json(), &expected),
        "session.json",
        thread_id,
        diagnostics,
        "complete events",
    )
}

fn repair_session_checkpoint(
    paths: &ThreadPaths,
    thread_id: ThreadId,
    suffix: &[RuntimeEvent],
    pre_session: &SessionState,
    diagnostics: &Arc<dyn DiagnosticSink>,
) -> bool {
    if !suffix.iter().any(affects_session) {
        return true;
    }
    let mut expected = pre_session.clone();
    for event in suffix {
        expected.apply(event);
    }
    record_write(
        write_json_atomic(&paths.session_json(), &expected),
        "session.json",
        thread_id,
        diagnostics,
        "checkpoint and event suffix",
    )
}

fn load_session_checkpoint_base(paths: &ThreadPaths) -> Result<SessionState> {
    read_json_opt::<SessionState>(&paths.session_json())
        .ok_or_else(|| ark_common::ArkError::store("session checkpoint is missing or invalid"))
}

fn affects_session(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::TurnStarted { .. }
            | RuntimeEvent::TurnCompleted { .. }
            | RuntimeEvent::TurnFailed { .. }
            | RuntimeEvent::TurnAborted { .. }
    )
}

fn same_session(left: &SessionState, right: &SessionState) -> bool {
    left.status == right.status
        && left.last_turn_id == right.last_turn_id
        && left.started_at == right.started_at
        && left.ended_at == right.ended_at
        && left.error == right.error
}

fn record_invalid_checkpoint(
    file: &str,
    thread_id: ThreadId,
    diagnostics: &Arc<dyn DiagnosticSink>,
) -> bool {
    diagnostics.record(
        StoreDiagnostic::new(
            DiagnosticKind::DerivedWriteFailed,
            format!("cannot recover {file}: checkpoint is missing or invalid"),
        )
        .for_thread(thread_id),
    );
    false
}

fn record_write(
    result: Result<()>,
    file: &str,
    thread_id: ThreadId,
    diagnostics: &Arc<dyn DiagnosticSink>,
    source: &str,
) -> bool {
    let success = result.is_ok();
    let (kind, detail) = match result {
        Ok(()) => (
            DiagnosticKind::DerivedRebuilt,
            format!("rebuilt {file} from {source}"),
        ),
        Err(err) => (
            DiagnosticKind::DerivedWriteFailed,
            format!("failed to rebuild {file} from {source}: {err}"),
        ),
    };
    diagnostics.record(StoreDiagnostic::new(kind, detail).for_thread(thread_id));
    success
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::{ItemId, TurnId};
    use ark_contract::item::ItemKind;
    use ark_contract::ThreadItem;

    fn record(item_id: ItemId, first_seq: u64, watermark_seq: u64, text: &str) -> ItemRow {
        let mut item = ThreadItem::new(TurnId::new(), ItemKind::Notice).with_text(text);
        item.item_id = item_id;
        ItemRow::Watermarked(ItemSnapshotRecord {
            first_seq,
            watermark_seq,
            item,
        })
    }

    #[test]
    fn canonical_items_fold_append_history_to_the_latest_snapshot() {
        let item_id = ItemId::new();
        let rows = vec![record(item_id, 1, 1, "old"), record(item_id, 1, 3, "new")];
        let canonical = canonical_item_records(rows).unwrap();
        assert_eq!(canonical.len(), 1);
        assert_eq!(canonical[0].first_seq, 1);
        assert_eq!(canonical[0].watermark_seq, 3);
        assert_eq!(canonical[0].item.text.as_deref(), Some("new"));
    }

    #[test]
    fn canonical_items_reject_legacy_rows() {
        let item = ThreadItem::new(TurnId::new(), ItemKind::Notice);
        assert!(canonical_item_records(vec![ItemRow::Bare(item)]).is_none());
    }
}
