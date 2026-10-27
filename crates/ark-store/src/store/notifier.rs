//! Index invalidation hook: notify the projector that a thread advanced.
//!
//! Design 7.4.2: the thread writer never waits for the index. It either sends a
//! delta into the projector's bounded queue or, when that queue is full, only
//! records the thread as dirty so the projector catches up from the log later.

use ark_common::ThreadId;

/// Non-blocking notification sink implemented by `ark-store`'s index projector.
pub trait IndexNotifier: Send + Sync {
    /// The thread's log is durable up to `to_seq`.
    ///
    /// Implementations must return immediately and must not propagate errors:
    /// the event log is already the source of truth.
    fn thread_advanced(&self, thread: ThreadId, to_seq: u64);
}

/// Notifier that ignores every update (used when indexing is disabled).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopNotifier;

impl IndexNotifier for NoopNotifier {
    fn thread_advanced(&self, _thread: ThreadId, _to_seq: u64) {}
}
