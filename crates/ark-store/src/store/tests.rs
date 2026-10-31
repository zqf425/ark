//! Shared test helpers for the store crate.
//!
//! `TempDir` wraps [`tempfile::TempDir`] so parallel tests in the same process
//! can never pick the same directory: the crate generates a random unique name
//! and removes the tree on drop, which is exactly what these tests need.

use ark_common::{ItemId, ThreadId, TurnId};
use ark_contract::RuntimeEvent;
use std::path::Path;

/// A unique temporary directory removed on drop.
pub(crate) struct TempDir {
    inner: tempfile::TempDir,
}

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let inner = tempfile::Builder::new()
            .prefix(&format!("ark-store-{tag}-"))
            .tempdir()
            .expect("temp dir is creatable");
        Self { inner }
    }

    pub fn path(&self) -> &Path {
        self.inner.path()
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
        auto_continuation: false,
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
