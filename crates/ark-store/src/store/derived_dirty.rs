//! Crash marker for event-derived thread projections.
//!
//! Presence is intentionally determined without parsing: a malformed marker
//! still proves that a previous append may have committed its authoritative
//! event without completing every derived write.

use crate::domain::layout::ThreadPaths;
use crate::domain::session_state::SessionState;
use crate::store::fs_util::{path_entry_exists, sync_parent_dir, write_json_atomic};
use ark_common::{ArkError, Result, TurnId};
use ark_contract::RuntimeEvent;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;

const VERSION: u8 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DerivedDirty {
    version: u8,
    planned_seq: u64,
    pre_current_turn: Option<TurnId>,
    pre_session: SessionState,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyDerivedDirty {
    planned_seq: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct RecoveryPlan {
    pub(crate) planned_seq: u64,
    pub(crate) pre_current_turn: Option<TurnId>,
    pub(crate) pre_session: SessionState,
}

/// Read a strictly versioned marker. Legacy markers are accepted only when the
/// complete pre-planned event prefix proves the missing turn context.
pub(crate) fn recovery_plan(
    paths: &ThreadPaths,
    events: &[RuntimeEvent],
    complete_history: bool,
) -> Option<RecoveryPlan> {
    let bytes = std::fs::read(paths.derived_dirty_json()).ok()?;
    if let Ok(marker) = serde_json::from_slice::<DerivedDirty>(&bytes) {
        let plan =
            (marker.version == VERSION && marker.planned_seq > 0).then_some(RecoveryPlan {
                planned_seq: marker.planned_seq,
                pre_current_turn: marker.pre_current_turn,
                pre_session: marker.pre_session,
            })?;
        return suffix_is_contiguous(events, plan.planned_seq).then_some(plan);
    }
    let legacy = serde_json::from_slice::<LegacyDerivedDirty>(&bytes).ok()?;
    if legacy.planned_seq == 0 || !complete_history {
        return None;
    }
    let prefix = events
        .iter()
        .take_while(|event| event.seq() < legacy.planned_seq)
        .cloned()
        .collect::<Vec<_>>();
    let contiguous = prefix.len() as u64 == legacy.planned_seq - 1
        && prefix
            .iter()
            .enumerate()
            .all(|(offset, event)| event.seq() == offset as u64 + 1);
    let pre_session = SessionState::replay(&prefix);
    (contiguous && suffix_is_contiguous(events, legacy.planned_seq)).then_some(RecoveryPlan {
        planned_seq: legacy.planned_seq,
        pre_current_turn: pre_session.last_turn_id,
        pre_session,
    })
}

fn suffix_is_contiguous(events: &[RuntimeEvent], planned_seq: u64) -> bool {
    let Some(start) = events.iter().position(|event| event.seq() == planned_seq) else {
        return false;
    };
    events[start..]
        .iter()
        .enumerate()
        .all(|(offset, event)| event.seq() == planned_seq + offset as u64)
}

/// Read only the planned sequence for the safe "append never landed" proof.
pub(crate) fn planned_seq(paths: &ThreadPaths) -> Option<u64> {
    let bytes = std::fs::read(paths.derived_dirty_json()).ok()?;
    if let Ok(marker) = serde_json::from_slice::<DerivedDirty>(&bytes) {
        return (marker.version == VERSION && marker.planned_seq > 0).then_some(marker.planned_seq);
    }
    let marker = serde_json::from_slice::<LegacyDerivedDirty>(&bytes).ok()?;
    (marker.planned_seq > 0).then_some(marker.planned_seq)
}

/// Whether any filesystem entry occupies the marker path.
pub(crate) fn exists(paths: &ThreadPaths) -> bool {
    path_entry_exists(&paths.derived_dirty_json())
}

/// Create the marker before an event append, preserving an older marker.
/// Returns true only when this call owns the marker and may clear it.
pub(crate) fn mark_if_clean(
    paths: &ThreadPaths,
    planned_seq: u64,
    pre_current_turn: Option<TurnId>,
    pre_session: &SessionState,
) -> Result<bool> {
    let path = paths.derived_dirty_json();
    if exists(paths) {
        sync_parent_dir(&path)?;
        return Ok(false);
    }
    let marker = DerivedDirty {
        version: VERSION,
        planned_seq,
        pre_current_turn,
        pre_session: pre_session.clone(),
    };
    write_json_atomic(&path, &marker)?;
    sync_parent_dir(&path)?;
    Ok(true)
}

/// Remove a recovered marker. Missing markers are already clean.
pub(crate) fn clear(paths: &ThreadPaths) -> Result<()> {
    let path = paths.derived_dirty_json();
    match std::fs::remove_file(&path) {
        Ok(()) => sync_parent_dir(&path),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ArkError::store(format!(
            "remove derived dirty marker failed for {}: {error}",
            path.display()
        ))),
    }
}
