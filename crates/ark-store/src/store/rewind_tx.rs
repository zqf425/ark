//! Crash-safe multi-file rewind transaction.
//!
//! The intention is durable before `events.jsonl` changes. Recovery compares
//! the current event bytes with the declared before/after hashes: unchanged
//! bytes abort the transaction, while replacement bytes drive an idempotent
//! rebuild of every derived file before the intention is removed.

use crate::domain::layout::ThreadPaths;
use crate::domain::session_state::SessionState;
use crate::store::event_log::parse_log;
use crate::store::fs_util::{path_entry_exists, sync_parent_dir, write_atomic, write_json_atomic};
use crate::store::writer_state::RewindRecord;
use ark_common::{ArkError, Result};
use ark_contract::RuntimeEvent;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::ErrorKind;

const VERSION: u8 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRewind {
    version: u8,
    transaction_id: uuid::Uuid,
    before_sha256: String,
    after_sha256: String,
    rewinds_after: Vec<RewindRecord>,
}

pub(crate) struct RewindTransaction {
    paths: ThreadPaths,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryOutcome {
    None,
    Aborted,
    Committed,
}

impl RewindTransaction {
    pub(crate) fn begin(
        paths: &ThreadPaths,
        after: &[u8],
        rewinds_after: &[RewindRecord],
    ) -> Result<Self> {
        recover(paths)?;
        let pending = PendingRewind {
            version: VERSION,
            transaction_id: uuid::Uuid::new_v4(),
            before_sha256: digest(&read_events(paths)?),
            after_sha256: digest(after),
            rewinds_after: rewinds_after.to_vec(),
        };
        write_json_atomic(&paths.rewind_pending_json(), &pending)?;
        sync_parent_dir(&paths.rewind_pending_json())?;
        Ok(Self {
            paths: paths.clone(),
        })
    }

    pub(crate) fn replace_events(&self, after: &[u8]) -> Result<()> {
        test_fail(&self.paths, TestStage::EventsWrite)?;
        write_atomic(&self.paths.events_jsonl(), after)?;
        sync_parent_dir(&self.paths.events_jsonl())
    }

    pub(crate) fn commit(self) -> Result<()> {
        test_fail(&self.paths, TestStage::DerivedRebuild)?;
        recover(&self.paths).map(|_| ())
    }
}

/// Recover a pending rewind by proving whether old or rewound events won.
pub(crate) fn recover(paths: &ThreadPaths) -> Result<RecoveryOutcome> {
    if !path_entry_exists(&paths.rewind_pending_json()) {
        return Ok(RecoveryOutcome::None);
    }
    let pending = read_pending(paths)?;
    let current = digest(&read_events(paths)?);
    if current == pending.before_sha256 {
        clear_pending(paths)?;
        return Ok(RecoveryOutcome::Aborted);
    }
    if current != pending.after_sha256 {
        return Err(ArkError::store(format!(
            "cannot recover rewind transaction {}: events.jsonl matches neither old nor rewound bytes",
            pending.transaction_id
        )));
    }
    let events = parse_rewound_events(paths)?;
    rebuild_derived(paths, &events, &pending.rewinds_after)?;
    clear_pending(paths)?;
    Ok(RecoveryOutcome::Committed)
}

fn read_pending(paths: &ThreadPaths) -> Result<PendingRewind> {
    let path = paths.rewind_pending_json();
    let bytes = std::fs::read(&path)
        .map_err(|error| ArkError::store(format!("read rewind intention failed: {error}")))?;
    let pending: PendingRewind = serde_json::from_slice(&bytes).map_err(|error| {
        ArkError::store(format!(
            "invalid rewind intention {}: {error}",
            path.display()
        ))
    })?;
    if pending.version != VERSION {
        return Err(ArkError::store(format!(
            "unsupported rewind intention version {}",
            pending.version
        )));
    }
    Ok(pending)
}

fn parse_rewound_events(paths: &ThreadPaths) -> Result<Vec<RuntimeEvent>> {
    let bytes = read_events(paths)?;
    let parsed = parse_log(&bytes);
    parsed.reject_malformed(&paths.events_jsonl())?;
    if parsed.truncate_at.is_some() {
        return Err(ArkError::store("rewound event log has a torn tail"));
    }
    Ok(parsed.events)
}

fn rebuild_derived(
    paths: &ThreadPaths,
    events: &[RuntimeEvent],
    rewinds: &[RewindRecord],
) -> Result<()> {
    crate::store::items_log::rewrite_items_from_events_blocking(paths, events)?;
    crate::store::items_log::rewrite_usage_from_events_blocking(paths, events)?;
    write_json_atomic(&paths.session_json(), &SessionState::replay(events))?;
    write_json_atomic(&paths.rewinds_json(), &rewinds)?;
    sync_parent_dir(&paths.rewinds_json())
}

fn clear_pending(paths: &ThreadPaths) -> Result<()> {
    let path = paths.rewind_pending_json();
    match std::fs::remove_file(&path) {
        Ok(()) => sync_parent_dir(&path),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ArkError::store(format!(
            "remove rewind intention failed for {}: {error}",
            path.display()
        ))),
    }
}

fn read_events(paths: &ThreadPaths) -> Result<Vec<u8>> {
    match std::fs::read(paths.events_jsonl()) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(ArkError::store(format!("read event log failed: {error}"))),
    }
}

fn digest(bytes: &[u8]) -> String {
    let value = Sha256::digest(bytes);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestStage {
    EventsWrite,
    DerivedRebuild,
}

#[cfg(not(test))]
#[derive(Clone, Copy)]
enum TestStage {
    EventsWrite,
    DerivedRebuild,
}

#[cfg(test)]
static TEST_FAILURES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, TestStage>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) fn fail_once(paths: &ThreadPaths, stage: TestStage) {
    TEST_FAILURES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(paths.dir().to_path_buf(), stage);
}

#[cfg(test)]
fn test_fail(paths: &ThreadPaths, stage: TestStage) -> Result<()> {
    let failures = TEST_FAILURES.get_or_init(Default::default);
    let mut failures = failures.lock().unwrap();
    if failures.get(paths.dir()) == Some(&stage) {
        failures.remove(paths.dir());
        return Err(ArkError::store("injected rewind transaction failure"));
    }
    Ok(())
}

#[cfg(not(test))]
fn test_fail(_paths: &ThreadPaths, _stage: TestStage) -> Result<()> {
    Ok(())
}
