//! Live turn bookkeeping: slot lifecycle, cancellation and steering handoff.
//!
//! Split out of `state.rs` so each file owns one concept (the registry seam vs
//! the application state the composition root fills in).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use ark_agent::{CancelToken, SteerQueue};
use ark_common::{ThreadId, TurnId};
use ark_contract::TurnSummary;
use chrono::Utc;

use super::{SteerError, TurnEnd, TurnRegistration};
/// How many terminal turns stay readable after they end.
///
/// Two callers legitimately read a finished turn: `GET
/// /v1/threads/{id}/turns/{turn_id}`, and a `/steer` sent before the client has
/// received the terminal event. Dropping the slot the instant it closes would
/// turn those answers from "completed"/"already finished" into "unknown". The
/// window therefore stays small: it is the only brake on growth, so a long-lived
/// process keeps at most `active + TERMINAL_TURNS_RETAINED` slots and answers the
/// documented not-found error for anything older.
pub(super) const TERMINAL_TURNS_RETAINED: usize = 8;

/// Live turn bookkeeping (state + the cancel handle used by `/interrupt`).
pub struct TurnRegistry {
    inner: Mutex<TurnRegistryState>,
    latest_by_thread: Mutex<HashMap<ThreadId, TurnId>>,
}

/// Slots plus the finish order used to reclaim them.
///
/// Both live behind one lock so eviction can never race the close that queued
/// the id: the map and the retention window stay consistent after every write.
struct TurnRegistryState {
    slots: HashMap<TurnId, TurnSlot>,
    /// Terminal turn ids in finish order; the front is evicted first.
    finished: VecDeque<TurnId>,
    shutting_down: bool,
}

impl TurnRegistryState {
    fn new() -> Self {
        Self {
            slots: HashMap::new(),
            finished: VecDeque::new(),
            shutting_down: false,
        }
    }
}

struct TurnSlot {
    summary: TurnSummary,
    cancel: CancelToken,
    steering: Arc<SteerQueue>,
}

impl Default for TurnRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TurnRegistryState::new()),
            latest_by_thread: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TurnRegistryState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Drop the oldest terminal slots once the retention window is exceeded.
    fn evict_finished(state: &mut TurnRegistryState) {
        while state.finished.len() > TERMINAL_TURNS_RETAINED {
            let Some(expired) = state.finished.pop_front() else {
                break;
            };
            state.slots.remove(&expired);
        }
    }

    /// Register a turn as running and hand back the handles its worker needs.
    ///
    /// Returns `None` when the thread already has an in-flight turn. The existing
    /// runtime rejects with `conflictPolicy: 'reject'` and answers 409, so the
    /// renderer keeps the message queued and retries once the running turn
    /// settles; accepting a second turn here would interleave two model loops
    /// over one event log.
    pub fn try_register(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        cancel: CancelToken,
    ) -> Option<TurnRegistration> {
        let mut guard = self.lock();
        if guard.shutting_down {
            return None;
        }
        let busy = guard
            .slots
            .values()
            .any(|slot| slot.summary.thread_id == thread_id && slot.summary.ended_at.is_none());
        if busy {
            return None;
        }
        let summary = TurnSummary {
            turn_id,
            thread_id,
            state: "running".to_string(),
            started_at: Utc::now(),
            ended_at: None,
            error: None,
        };
        let steering = Arc::new(SteerQueue::new());
        guard.slots.insert(
            turn_id,
            TurnSlot {
                summary: summary.clone(),
                cancel,
                steering: Arc::clone(&steering),
            },
        );
        self.latest_by_thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(thread_id, turn_id);
        Some(TurnRegistration { summary, steering })
    }

    /// Close the registration gate under the same lock as slot insertion.
    ///
    /// A registration that wins this lock is visible to the subsequent cancel
    /// sweep; one that loses is rejected instead of escaping the drain.
    pub fn shutdown(&self) {
        self.lock().shutting_down = true;
    }

    /// The in-flight turn of a thread, if any.
    pub fn active_for(&self, thread_id: ThreadId) -> Option<TurnSummary> {
        self.lock()
            .slots
            .values()
            .find(|slot| slot.summary.thread_id == thread_id && slot.summary.ended_at.is_none())
            .map(|slot| slot.summary.clone())
    }

    /// Every thread that currently has an in-flight turn.
    ///
    /// One lock read for the whole list response, so the busy marker costs one
    /// allocation per call instead of one registry probe per row.
    pub fn active_thread_ids(&self) -> std::collections::HashSet<ThreadId> {
        self.lock()
            .slots
            .values()
            .filter(|slot| slot.summary.ended_at.is_none())
            .map(|slot| slot.summary.thread_id)
            .collect()
    }

    /// The summary of a known turn, terminal or running.
    ///
    /// A terminal record only lives for the [`TERMINAL_TURNS_RETAINED`] window,
    /// so `None` reads as "not running and no longer retained", which is the same
    /// not-found the route already documents for an unknown turn.
    pub fn get(&self, turn_id: TurnId) -> Option<TurnSummary> {
        self.lock()
            .slots
            .get(&turn_id)
            .map(|slot| slot.summary.clone())
    }

    /// The newest process-local lifecycle epoch registered for this thread.
    pub fn latest_for(&self, thread_id: ThreadId) -> Option<TurnId> {
        self.latest_by_thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&thread_id)
            .copied()
    }

    /// Reclaim the lifecycle epoch marker of a durably deleted thread.
    ///
    /// `latest_by_thread` only ever grows on registration; without this call a
    /// long-lived process keeps one `(ThreadId, TurnId)` entry per thread it
    /// ever created. Only the delete route calls it, after the durable delete
    /// succeeded: clearing earlier would break the `is_latest_for` ABA guard.
    /// Slots are a separate lifecycle and stay untouched.
    pub fn forget_thread(&self, thread_id: ThreadId) {
        self.latest_by_thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&thread_id);
    }

    /// True when `turn_id` is the newest turn registered for this thread.
    /// Lifecycle handlers hold `workspace_gate`, so registration cannot advance
    /// this marker between their check and continuation suppression.
    pub fn is_latest_for(&self, thread_id: ThreadId, turn_id: TurnId) -> bool {
        self.latest_for(thread_id) == Some(turn_id)
    }

    /// The queue the reactor must drain for this still-running turn.
    pub fn steering_for(&self, turn_id: TurnId) -> Option<Arc<SteerQueue>> {
        self.lock()
            .slots
            .get(&turn_id)
            .filter(|slot| slot.summary.ended_at.is_none())
            .map(|slot| Arc::clone(&slot.steering))
    }

    /// Queue a correction for the exact active turn and report saturation
    /// instead of dropping it silently. Attachment ids ride the same entry;
    /// the route authorizes them against the thread before calling this.
    pub fn enqueue_steering(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        text: &str,
        attachment_ids: Vec<ark_common::AttachmentId>,
    ) -> std::result::Result<TurnSummary, SteerError> {
        let guard = self.lock();
        let slot = guard.slots.get(&turn_id).ok_or(SteerError::NotFound)?;
        if slot.summary.thread_id != thread_id {
            return Err(SteerError::NotFound);
        }
        if slot.summary.ended_at.is_some() {
            return Err(SteerError::Finished);
        }
        if !slot.steering.push_with_attachments(text, attachment_ids) {
            return Err(SteerError::QueueFull);
        }
        Ok(slot.summary.clone())
    }

    /// Trip the turn's cancel token, if the turn is still known.
    pub fn cancel(&self, turn_id: TurnId) -> Option<TurnSummary> {
        let mut guard = self.lock();
        let slot = guard.slots.get_mut(&turn_id)?;
        slot.cancel.cancel();
        slot.summary.state = "aborting".to_string();
        Some(slot.summary.clone())
    }

    /// Atomically cancel an active turn owned by `thread_id`.
    ///
    /// Ownership and liveness are checked under the same lock as the state
    /// mutation, so a turn that finishes concurrently cannot be moved back from
    /// a terminal state to `aborting`.
    pub fn cancel_active(&self, thread_id: ThreadId, turn_id: TurnId) -> Option<TurnSummary> {
        let mut guard = self.lock();
        let slot = guard.slots.get_mut(&turn_id)?;
        if slot.summary.thread_id != thread_id || slot.summary.ended_at.is_some() {
            return None;
        }
        slot.cancel.cancel();
        slot.summary.state = "aborting".to_string();
        Some(slot.summary.clone())
    }

    /// Record the terminal state reported by the runner and schedule reclaim.
    ///
    /// The slot is not dropped in place: a client may still be reading this turn
    /// over HTTP when the terminal event lands. It is queued for eviction
    /// instead, so finished turns stay answerable for a bounded window and then
    /// leave the map. Without this the registry kept one slot per turn the
    /// process ever ran, each holding a `SteerQueue` and a cancel token.
    pub fn finish(&self, turn_id: TurnId, end: &TurnEnd) {
        let mut guard = self.lock();
        let Some(slot) = guard.slots.get_mut(&turn_id) else {
            return;
        };
        let (state, error) = match end {
            TurnEnd::Completed | TurnEnd::CompletedWithPendingCompletions => ("completed", None),
            TurnEnd::Failed {
                message,
                model_request_failed: _,
            } => ("failed", Some(message.clone())),
            TurnEnd::Aborted(reason) => ("aborted", Some(reason.clone())),
        };
        slot.summary.state = state.to_string();
        slot.summary.error = error;
        // The reactor and the supervisor can both settle one turn. A second
        // close must not queue the id twice, or the duplicate would evict a
        // still-relevant record one turn too early.
        let first_close = slot.summary.ended_at.is_none();
        slot.summary.ended_at = Some(Utc::now());
        if first_close {
            guard.finished.push_back(turn_id);
            Self::evict_finished(&mut guard);
        }
    }

    /// Cancel every in-flight turn with the ordinary interrupt reason.
    pub fn cancel_all(&self) {
        self.cancel_all_with_reason(ark_common::CancelReason::UserInterrupt.as_durable_str());
    }

    /// Cancel every in-flight turn with one durable process-owned reason.
    pub fn cancel_all_with_reason(&self, reason: &str) {
        for slot in self.lock().slots.values_mut() {
            slot.cancel.cancel_with_reason(reason);
            if slot.summary.ended_at.is_none() {
                slot.summary.state = "aborting".to_string();
            }
        }
    }

    pub fn active(&self) -> usize {
        self.lock()
            .slots
            .values()
            .filter(|slot| slot.summary.ended_at.is_none())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_agent::CancelToken;
    use ark_common::TurnId;

    #[test]
    fn finished_turns_are_reclaimed_after_the_retention_window() {
        let registry = TurnRegistry::new();
        let thread = ThreadId::new();
        let turns: Vec<TurnId> = (0..TERMINAL_TURNS_RETAINED + 3)
            .map(|_| {
                let turn = TurnId::new();
                registry
                    .try_register(thread, turn, CancelToken::new())
                    .expect("the previous turn already ended");
                registry.finish(turn, &TurnEnd::Completed);
                turn
            })
            .collect();

        // The registry tracks live work, not the transcript: only the newest
        // terminal turns stay addressable, so a long-running process cannot keep
        // one slot (queue plus cancel token) per turn it ever ran.
        assert_eq!(registry.lock().slots.len(), TERMINAL_TURNS_RETAINED);
        let evicted = turns.len() - TERMINAL_TURNS_RETAINED;
        for turn in turns.iter().take(evicted) {
            assert!(
                registry.get(*turn).is_none(),
                "a turn older than the window must not linger"
            );
        }
        for turn in turns.iter().skip(evicted) {
            assert!(
                registry.get(*turn).is_some(),
                "the window keeps recent terminal turns readable"
            );
        }
    }

    #[test]
    fn forget_thread_reclaims_only_the_deleted_threads_epoch_marker() {
        let registry = TurnRegistry::new();
        let doomed = ThreadId::new();
        let kept_a = ThreadId::new();
        let kept_b = ThreadId::new();
        let doomed_turn = TurnId::new();
        let kept_a_turn = TurnId::new();
        let kept_b_turn = TurnId::new();
        for (thread, turn) in [
            (doomed, doomed_turn),
            (kept_a, kept_a_turn),
            (kept_b, kept_b_turn),
        ] {
            registry
                .try_register(thread, turn, CancelToken::new())
                .expect("each thread is idle");
        }

        registry.forget_thread(doomed);

        assert_eq!(
            registry.latest_for(doomed),
            None,
            "the deleted thread's epoch marker must be reclaimed"
        );
        assert_eq!(registry.latest_for(kept_a), Some(kept_a_turn));
        assert_eq!(registry.latest_for(kept_b), Some(kept_b_turn));
        // Epoch bookkeeping is separate from slot retention.
        assert!(registry.get(doomed_turn).is_some());
    }
}
