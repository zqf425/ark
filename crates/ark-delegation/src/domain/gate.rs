//! Concurrency budget for child runs (design §7.8).
//!
//! Two numbers decide what happens to a spawn:
//!
//! * `limit` — children allowed to *run* at once;
//! * `max_queued` — children allowed to *wait* for a slot.
//!
//! Anything beyond both fails fast instead of queueing without bound. That is the
//! mainline behaviour as well: an unbounded backlog turns one busy parent into a
//! memory leak plus a pile of model requests nobody is waiting for any more.
//!
//! The gate is generic rather than per-parent: the runtime passes the policy in
//! on every acquire, so a settings change applies to the next child without the
//! gate holding a stale copy of the limit.

use ark_common::{ArkError, ArkErrorCode, Result};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::oneshot;

/// Outcome of a failed acquire.
pub const ERR_QUEUE_FULL: &str = "delegation child-run queue is full";

/// Build the error a saturated spawn returns.
pub fn acquire_error(limit: usize, max_queued: usize) -> ArkError {
    ArkError::new(
        ArkErrorCode::Conflict,
        format!(
            "{ERR_QUEUE_FULL} (max_concurrency={limit}, max_queued={max_queued}); \
             wait for a running child to finish or raise the limit in Settings"
        ),
    )
}

#[derive(Default)]
struct Inner {
    active: usize,
    waiters: VecDeque<oneshot::Sender<()>>,
}

/// Slot gate shared by every child spawn in the process.
pub struct SlotGate {
    inner: Mutex<Inner>,
}

impl Default for SlotGate {
    fn default() -> Self {
        Self::new()
    }
}

impl SlotGate {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Children holding a slot right now.
    pub fn active(&self) -> usize {
        self.lock().active
    }

    /// Children waiting for a slot right now.
    pub fn queued(&self) -> usize {
        self.lock().waiters.len()
    }

    /// Take a slot, waiting in the queue if all slots are busy.
    ///
    /// Returns [`ERR_QUEUE_FULL`]-shaped `Conflict` when both the slots and the
    /// queue are saturated, so a parent turn gets an actionable message instead
    /// of hanging.
    pub async fn acquire(self: &Arc<Self>, limit: usize, max_queued: usize) -> Result<SlotGuard> {
        let limit = limit.max(1);
        let waiter = {
            let mut inner = self.lock();
            if inner.active < limit {
                inner.active += 1;
                None
            } else if inner.waiters.len() < max_queued {
                let (tx, rx) = oneshot::channel();
                inner.waiters.push_back(tx);
                Some(rx)
            } else {
                return Err(acquire_error(limit, max_queued));
            }
        };

        if let Some(rx) = waiter {
            // The releaser transfers its slot to us, so `active` is unchanged.
            if rx.await.is_err() {
                // The gate was dropped mid-wait; nothing to release.
                return Err(ArkError::new(
                    ArkErrorCode::RuntimeUnavailable,
                    "delegation slot gate closed while waiting",
                ));
            }
        }

        Ok(SlotGuard {
            gate: Arc::clone(self),
        })
    }

    /// Hand the slot to the next waiter, or free it.
    fn release(&self) {
        let mut inner = self.lock();
        while let Some(waiter) = inner.waiters.pop_front() {
            // A waiter whose future was dropped (cancelled parent) no longer
            // receives; the slot must not be lost to a dead entry.
            if waiter.send(()).is_ok() {
                return;
            }
        }
        inner.active = inner.active.saturating_sub(1);
    }
}

/// Holds one slot; releases on drop, including on cancel and panic.
pub struct SlotGuard {
    gate: Arc<SlotGate>,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.gate.release();
    }
}

impl std::fmt::Debug for SlotGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SlotGuard")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn slots_are_reused_after_drop() {
        let gate = Arc::new(SlotGate::new());
        let first = gate.acquire(1, 4).await.unwrap();
        assert_eq!(gate.active(), 1);
        drop(first);
        assert_eq!(gate.active(), 0);
        let second = gate.acquire(1, 4).await.unwrap();
        assert_eq!(gate.active(), 1);
        drop(second);
    }

    #[tokio::test]
    async fn a_saturated_gate_and_queue_fail_fast() {
        let gate = Arc::new(SlotGate::new());
        let _held = gate.acquire(1, 0).await.unwrap();
        let err = gate.acquire(1, 0).await.unwrap_err();
        assert_eq!(err.code, ArkErrorCode::Conflict);
        assert!(err.message.contains("queue is full"), "{}", err.message);
    }

    #[tokio::test]
    async fn a_waiter_is_woken_when_a_slot_frees() {
        let gate = Arc::new(SlotGate::new());
        let held = gate.acquire(1, 4).await.unwrap();

        let waiting = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move {
                let guard = gate.acquire(1, 4).await.unwrap();
                // Hold briefly so the assertion below sees a transferred slot.
                tokio::time::sleep(Duration::from_millis(20)).await;
                drop(guard);
            })
        };

        // Let the waiter register, then free the slot.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(gate.queued(), 1);
        drop(held);

        waiting.await.unwrap();
        assert_eq!(gate.active(), 0);
    }

    #[tokio::test]
    async fn a_cancelled_waiter_does_not_swallow_the_slot() {
        // A parent turn that is interrupted while queued must not leak the slot.
        let gate = Arc::new(SlotGate::new());
        let held = gate.acquire(1, 4).await.unwrap();

        let waiting = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { gate.acquire(1, 4).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        waiting.abort();
        let _ = waiting.await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        drop(held);
        // The dead waiter must be skipped and the slot returned to the pool.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(gate.active(), 0);
        let again = gate.acquire(1, 4).await.unwrap();
        assert_eq!(gate.active(), 1);
        drop(again);
    }

    #[tokio::test]
    async fn concurrent_acquires_respect_the_limit() {
        let gate = Arc::new(SlotGate::new());
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let gate = Arc::clone(&gate);
            tasks.push(tokio::spawn(async move {
                let guard = gate.acquire(2, 32).await.unwrap();
                assert!(gate.active() <= 2, "limit must hold: {}", gate.active());
                tokio::time::sleep(Duration::from_millis(5)).await;
                drop(guard);
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(gate.active(), 0);
    }
}
