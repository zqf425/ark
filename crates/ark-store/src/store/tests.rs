//! Shared test helpers for the store crate.
//!
//! `TempDir` is a hand-rolled equivalent of the `tempfile` crate: the crate
//! must not grow a C-compiling dependency, and a unique directory name plus a
//! `Drop` cleanup covers everything the tests need.

use ark_common::{ItemId, ThreadId, TurnId};
use ark_contract::RuntimeEvent;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory removed on drop.
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("ark-store-{tag}-{}-{unique}", std::process::id()));
        if path.exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
        std::fs::create_dir_all(&path).expect("temp dir is creatable");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Minimal event with a caller-chosen `seq`.
pub(crate) fn event_at(thread: ThreadId, seq: u64) -> RuntimeEvent {
    RuntimeEvent::Heartbeat {
        seq,
        thread_id: thread,
        timestamp: chrono::Utc::now(),
    }
}

/// A `TurnStarted` event, useful when ordering matters.
pub(crate) fn turn_started(thread: ThreadId, turn: TurnId, seq: u64) -> RuntimeEvent {
    RuntimeEvent::TurnStarted {
        seq,
        thread_id: thread,
        turn_id: turn,
        timestamp: chrono::Utc::now(),
    }
}

/// An assistant text delta for a fixed item.
pub(crate) fn text_delta(
    thread: ThreadId,
    turn: TurnId,
    item: ItemId,
    seq: u64,
    delta: &str,
) -> RuntimeEvent {
    RuntimeEvent::AssistantTextDelta {
        seq,
        thread_id: thread,
        turn_id: turn,
        item_id: item,
        timestamp: chrono::Utc::now(),
        delta: delta.to_string(),
    }
}
