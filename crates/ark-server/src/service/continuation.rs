//! Debounced parent continuations for background child completions.
//!
//! Durable `child_completed` events remain the source of truth. This supervisor
//! only coalesces process-local wakeups and starts the existing parent turn path.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use ark_agent::{CancelToken, CompletionInbox};
use ark_common::{ThreadId, TurnId};
use ark_contract::RuntimeEvent;

use crate::state::{AppState, TurnJob, TurnSource};

/// Window in which completions for one parent are folded into one turn.
pub const AUTO_CONTINUE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Process-local per-thread debounce and suppression state.
pub struct ContinuationRegistry {
    inner: Mutex<ContinuationState>,
    next_generation: AtomicU64,
}

#[derive(Default)]
struct ContinuationState {
    pending: HashMap<ThreadId, (u64, CancelToken)>,
    suppressed: HashSet<ThreadId>,
}

impl Default for ContinuationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ContinuationRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ContinuationState::default()),
            next_generation: AtomicU64::new(1),
        }
    }

    /// Reserve the thread once. Further child completions join this debounce.
    pub fn try_schedule(&self, thread: ThreadId) -> Option<(u64, CancelToken)> {
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if state.suppressed.contains(&thread) || state.pending.contains_key(&thread) {
            return None;
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let cancel = CancelToken::new();
        state.pending.insert(thread, (generation, cancel.clone()));
        Some((generation, cancel))
    }

    pub fn is_current(&self, thread: ThreadId, generation: u64) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending
            .get(&thread)
            .is_some_and(|(current, _)| *current == generation)
    }

    pub fn finish(&self, thread: ThreadId, generation: u64) {
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if state
            .pending
            .get(&thread)
            .is_some_and(|(current, _)| *current == generation)
        {
            state.pending.remove(&thread);
        }
    }

    /// Cancel and forget a pending debounce. Safe to call repeatedly.
    pub fn cancel(&self, thread: ThreadId) -> bool {
        let removed = self
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending
            .remove(&thread);
        if let Some((_, token)) = removed {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Cancel pending work and reject later wakeups until explicit user input.
    /// Returns whether the thread was already suppressed, allowing a failed
    /// lifecycle mutation to restore the exact prior state.
    pub fn suppress(&self, thread: ThreadId) -> bool {
        let (was_suppressed, removed) = {
            let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            let was_suppressed = !state.suppressed.insert(thread);
            (was_suppressed, state.pending.remove(&thread))
        };
        if let Some((_, token)) = removed {
            token.cancel();
        }
        was_suppressed
    }

    /// Explicit user input begins a new lifecycle epoch for this thread.
    pub fn allow(&self, thread: ThreadId) {
        let removed = {
            let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            state.suppressed.remove(&thread);
            state.pending.remove(&thread)
        };
        if let Some((_, token)) = removed {
            token.cancel();
        }
    }

    pub fn cancel_all(&self) {
        self.cancel_all_with_reason("turn interrupted");
    }

    /// Cancel every pending debounce with a process-owned durable reason.
    pub fn cancel_all_with_reason(&self, reason: &str) {
        let pending = std::mem::take(
            &mut self
                .inner
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pending,
        );
        for (_, token) in pending.into_values() {
            token.cancel_with_reason(reason);
        }
    }

    #[cfg(test)]
    fn contains(&self, thread: ThreadId) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending
            .contains_key(&thread)
    }
}

/// Schedule one debounced continuation wakeup when the live policy permits it.
pub async fn schedule_auto_continuation(state: AppState, thread_id: ThreadId) {
    schedule_with_delay(state, thread_id, AUTO_CONTINUE_DEBOUNCE).await;
}

/// Apply the live auto-continue switch to process-local scheduling state.
///
/// Disabling cancels every pending debounce immediately. Enabling scans durable
/// inboxes so notifications that arrived while the switch was off are not lost.
pub async fn sync_auto_continue_policy(state: AppState) {
    sync_auto_continue_policy_inner(state, |_| {}).await;
}

async fn sync_auto_continue_policy_inner(state: AppState, before_suppress: impl Fn(ThreadId)) {
    if !policy_allows_auto_continue(&state) {
        state.continuations.cancel_all();
        return;
    }
    let threads = match state.store.list_threads().await {
        Ok(threads) => threads,
        Err(error) => {
            tracing::warn!(%error, "could not scan threads for automatic continuations");
            return;
        }
    };
    for thread in threads {
        let thread_id = thread.thread_id;
        let observed_epoch = state.turns.latest_for(thread_id);
        // A running parent owns this lifecycle epoch. Its completion hook will
        // inspect the durable inbox, so an older persisted abort must not
        // suppress work that arrived during the newer active turn.
        if state.turns.active_for(thread_id).is_some() {
            continue;
        }
        match pending_completion_state(&state, thread_id).await {
            PendingCompletionState::Empty => continue,
            PendingCompletionState::Retry => {
                // Reserve a retrying slot so a transient read failure cannot lose
                // the startup/lag-recovery wakeup.
                schedule_auto_continuation(state.clone(), thread_id).await;
            }
            PendingCompletionState::Pending => {
                if interrupted_user_lifecycle(&state, thread_id).await
                    || auto_continue_paused_by_model_failures(&state, thread_id).await
                {
                    before_suppress(thread_id);
                    suppress_if_epoch_is_idle(&state, thread_id, observed_epoch).await;
                } else {
                    schedule_auto_continuation(state.clone(), thread_id).await;
                }
            }
        }
    }
}

/// Apply a durable interrupt only if no explicit user lifecycle superseded the
/// scan. User registration and this commit point share `workspace_gate`.
async fn suppress_if_epoch_is_idle(
    state: &AppState,
    thread_id: ThreadId,
    observed_epoch: Option<TurnId>,
) {
    let _workspace_guard = state.workspace_gate.lock().await;
    if state.turns.active_for(thread_id).is_none()
        && state.turns.latest_for(thread_id) == observed_epoch
    {
        state.continuations.suppress(thread_id);
    }
}

/// Restore an interrupt across process restarts from durable turn state.
///
/// The newest terminal parent event defines the lifecycle epoch. A real abort
/// suppresses automatic work until a newer explicit user turn completes. The
/// startup repair reason is excluded because a process crash is not user intent.
async fn interrupted_user_lifecycle(state: &AppState, thread_id: ThreadId) -> bool {
    let events = match state.store.replay_interaction_events(thread_id).await {
        Ok(events) => events,
        Err(error) => {
            // Suppression is a safety property. If the durable lifecycle cannot
            // be read, do not risk starting an unattended model turn; a later
            // policy sync or completion wakeup can retry after storage recovers.
            tracing::warn!(thread = %thread_id, %error, "could not inspect continuation suppression; failing closed");
            return true;
        }
    };
    events.iter().rev().find_map(|event| match event {
        RuntimeEvent::TurnAborted { reason, .. } => Some(
            reason != "runtime restarted before turn could finish"
                && reason != crate::state::SHUTDOWN_CANCEL_REASON,
        ),
        RuntimeEvent::TurnCompleted { .. } | RuntimeEvent::TurnFailed { .. } => Some(false),
        _ => None,
    }) == Some(true)
}

fn policy_allows_auto_continue(state: &AppState) -> bool {
    let settings = state.config.snapshot().subagents;
    settings.enabled && settings.auto_continue
}

/// Circuit breaker: consecutive automatic continuations that failed on the
/// model request itself (gateway outages) pause this thread's automatic work.
/// Without it, a failing provider loops: the notification stays unacknowledged
/// because the parent turn failed, and every retry re-delivers it. A user turn
/// or any other terminal event resets the streak; nothing is scheduled while
/// the breaker holds, so no model calls are produced by the supervisor.
const MAX_CONSECUTIVE_AUTO_MODEL_FAILURES: usize = 2;

async fn auto_continue_paused_by_model_failures(state: &AppState, thread_id: ThreadId) -> bool {
    match state.store.replay_interaction_events(thread_id).await {
        Ok(events) => {
            let streak = ark_agent::consecutive_auto_model_request_failures(&events);
            if streak >= MAX_CONSECUTIVE_AUTO_MODEL_FAILURES {
                tracing::warn!(
                    thread = %thread_id,
                    streak,
                    "auto-continuation paused after consecutive model request failures; requires a user turn or a non-model terminal event to reset"
                );
                true
            } else {
                false
            }
        }
        Err(error) => {
            // Fail closed: an unreadable lifecycle log must not risk an
            // unattended model turn (same policy as interrupt suppression).
            tracing::warn!(thread = %thread_id, %error, "could not inspect model-failure streak; failing closed");
            true
        }
    }
}

async fn schedule_with_delay(state: AppState, thread_id: ThreadId, delay: Duration) {
    if state.is_shutting_down() || !policy_allows_auto_continue(&state) {
        return;
    }
    let Ok(thread) = state.store.get_thread(thread_id).await else {
        return;
    };
    if thread.archived {
        return;
    }
    let Some((generation, cancel)) = state.continuations.try_schedule(thread_id) else {
        return;
    };

    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(delay) => {
                loop {
                    wait_until_idle(&state, thread_id, generation, &cancel).await;
                    if !start_if_eligible(&state, thread_id, generation, cancel.clone()).await {
                        break;
                    }
                    // Keep this generation reserved while transient store errors
                    // recover; otherwise the one durable wakeup would be lost.
                    tokio::select! {
                        _ = tokio::time::sleep(AUTO_CONTINUE_DEBOUNCE) => {}
                        _ = cancel.cancelled() => break,
                    }
                }
            }
            _ = cancel.cancelled() => {}
        }
        state.continuations.finish(thread_id, generation);
    });
}

/// Keep the single pending slot while an existing turn settles. A completion
/// during that turn is not lost and cannot reserve a second continuation.
async fn wait_until_idle(
    state: &AppState,
    thread_id: ThreadId,
    generation: u64,
    cancel: &CancelToken,
) {
    while state.turns.active_for(thread_id).is_some()
        && !cancel.is_cancelled()
        && state.continuations.is_current(thread_id, generation)
        && !state.is_shutting_down()
        && policy_allows_auto_continue(state)
    {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            _ = cancel.cancelled() => break,
        }
    }
}

/// Recheck every eligibility condition under the same gate as ordinary turn
/// registration. This closes races with user turns, archive and deletion.
/// Returns `true` only when a transient completion-inbox read should be retried
/// while retaining the current per-thread scheduling slot.
async fn start_if_eligible(
    state: &AppState,
    thread_id: ThreadId,
    generation: u64,
    cancel: CancelToken,
) -> bool {
    // Read durable state before entering the registration gate. Archive/delete
    // handlers suppress this generation while holding the same gate, so the
    // in-gate `is_current` check closes the race without awaiting file I/O.
    let Ok(thread) = state.store.get_thread(thread_id).await else {
        return false;
    };
    if thread.archived {
        return false;
    }
    match pending_completion_state(state, thread_id).await {
        PendingCompletionState::Pending => {}
        PendingCompletionState::Empty => return false,
        PendingCompletionState::Retry => return true,
    }
    let observed_epoch = state.turns.latest_for(thread_id);
    if interrupted_user_lifecycle(state, thread_id).await {
        suppress_if_epoch_is_idle(state, thread_id, observed_epoch).await;
        return false;
    }
    if auto_continue_paused_by_model_failures(state, thread_id).await {
        return false;
    }

    let _workspace_guard = state.workspace_gate.lock().await;
    if cancel.is_cancelled()
        || !state.continuations.is_current(thread_id, generation)
        || state.is_shutting_down()
        || !policy_allows_auto_continue(state)
        || state.turns.active_for(thread_id).is_some()
    {
        return false;
    }

    let turn_id = TurnId::new();
    let Some(registration) = state.turns.try_register(thread_id, turn_id, cancel.clone()) else {
        return false;
    };
    state.continuations.finish(thread_id, generation);
    let job = TurnJob {
        thread_id,
        turn_id,
        source: TurnSource::AutoContinuation,
        cancel,
        steering: registration.steering,
    };
    crate::api::handlers::turns::spawn_turn(state.clone(), job);
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingCompletionState {
    Pending,
    Empty,
    Retry,
}

async fn pending_completion_state(state: &AppState, thread_id: ThreadId) -> PendingCompletionState {
    #[cfg(test)]
    if tests::take_completion_read_failure(thread_id) {
        return PendingCompletionState::Retry;
    }

    let completion = state.store.replay_child_completion_events(thread_id).await;
    let interaction = state.store.replay_interaction_events(thread_id).await;
    match (completion, interaction) {
        // The delivery-attempt accounting needs the `turn_started` markers from
        // the interaction stream, so both narrow queries merge by `seq` before
        // projecting with the per-notification bound. The notification stays
        // unacknowledged, so user turns still consume it.
        (Ok(mut events), Ok(lifecycle)) => {
            events.extend(lifecycle);
            events.sort_by_key(|event| event.seq());
            if CompletionInbox::from_events_for_auto_continuation(&events)
                .notification_ids()
                .is_empty()
            {
                PendingCompletionState::Empty
            } else {
                PendingCompletionState::Pending
            }
        }
        (Err(error), _) | (_, Err(error)) => {
            tracing::warn!(thread = %thread_id, %error, "could not inspect child completion inbox; retaining scheduled retry");
            PendingCompletionState::Retry
        }
    }
}

#[cfg(test)]
mod tests;
