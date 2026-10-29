//! HTTP-resolvable approval and user-input gates.
//!
//! The reactor owns event ordering and cancellation; this bridge only pairs a
//! currently registered request with its HTTP reply. `prepare_*` is synchronous
//! and inserts the map entry before returning, so a gate id is resolvable the
//! moment the reactor can append its `*_requested` event — no 404 window. The
//! returned future owns a drop guard that removes its own registration, so
//! cancelling or aborting a turn cannot leak a permanently pending gate. The
//! guard removes by generation, not just id, so a stale waiter dropped after
//! the id was re-registered cannot delete the new registration.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use ark_agent::{ApprovalReply, ApprovalRequest, GateBroker, GateFuture, UserInputRequest};
use ark_common::{ArkError, Result};
use tokio::sync::oneshot;

/// Monotonic registration counter: every `prepare_*` bumps it, so two
/// registrations under the same id (the ABA case) never share a generation.
static NEXT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_generation() -> u64 {
    NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One typed waiters map: id -> (generation, reply sender). Shared by the HTTP
/// resolution side and the drop guards of the in-flight wait futures.
type Waiters<R> = Arc<Mutex<HashMap<String, (u64, oneshot::Sender<R>)>>>;

/// Removes exactly this registration (id + generation) when the future is
/// dropped before resolution. Owned by the wait future, so cancel, abort or
/// append failure always cleans up, and only cleans up its own entry.
struct RegistrationGuard<R> {
    waiters: Waiters<R>,
    id: String,
    generation: u64,
}

impl<R> Drop for RegistrationGuard<R> {
    fn drop(&mut self) {
        let mut waiters = lock(&self.waiters);
        if waiters
            .get(&self.id)
            .is_some_and(|(generation, _)| *generation == self.generation)
        {
            waiters.remove(&self.id);
        }
    }
}

/// Insert a registration, refusing a still-pending duplicate id.
fn register<R>(waiters: &Waiters<R>, id: &str, sender: oneshot::Sender<R>) -> Result<u64> {
    let generation = next_generation();
    let mut map = lock(waiters);
    if map.contains_key(id) {
        return Err(ArkError::conflict("gate id is already pending"));
    }
    map.insert(id.to_string(), (generation, sender));
    Ok(generation)
}

/// Insert a registration and build its wait future. The map entry exists
/// before this returns; the future owns the guard, so a cancelled, aborted or
/// never-polled future still removes its own registration.
fn prepare<R: Send + 'static>(
    waiters: &Waiters<R>,
    id: String,
    cancelled_message: &'static str,
) -> Result<GateFuture<R>> {
    let (sender, receiver) = oneshot::channel();
    let generation = register(waiters, &id, sender)?;
    let guard = RegistrationGuard {
        waiters: Arc::clone(waiters),
        id,
        generation,
    };
    Ok(Box::pin(async move {
        // `guard` lives in this future: resolved via HTTP the entry is already
        // gone and dropping the guard is a no-op; cancelled early the guard's
        // Drop removes the entry.
        let _guard = guard;
        receiver
            .await
            .map_err(|_| ArkError::internal(cancelled_message))
    }))
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

/// In-process rendezvous used by the HTTP handlers and the parent reactor.
///
/// Bounded by the number of active reactor gates: every request holds exactly
/// one `oneshot` sender and cancellation drops it. No polling loop, no second
/// state machine.
#[derive(Default)]
pub struct InteractiveGates {
    approvals: Waiters<ApprovalReply>,
    inputs: Waiters<String>,
}

impl InteractiveGates {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve a known approval. `false` means it was already resolved,
    /// cancelled, or never existed.
    pub fn resolve_approval(&self, id: &str, approved: bool) -> bool {
        let reply = if approved {
            ApprovalReply::Approved
        } else {
            ApprovalReply::denied("rejected by the user")
        };
        lock(&self.approvals)
            .remove(id)
            .map(|(_, sender)| sender.send(reply).is_ok())
            .unwrap_or(false)
    }

    /// Resolve a known free-form input request.
    pub fn resolve_user_input(&self, id: &str, answer: String) -> bool {
        lock(&self.inputs)
            .remove(id)
            .map(|(_, sender)| sender.send(answer).is_ok())
            .unwrap_or(false)
    }

    #[cfg(test)]
    fn pending_approvals(&self) -> usize {
        lock(&self.approvals).len()
    }

    /// IDs whose one-shot approval receivers are still owned by this process.
    pub fn pending_approval_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = lock(&self.approvals).keys().cloned().collect();
        ids.sort();
        ids
    }

    /// IDs whose one-shot input receivers are still owned by this process.
    pub fn pending_user_input_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = lock(&self.inputs).keys().cloned().collect();
        ids.sort();
        ids
    }
}

impl GateBroker for InteractiveGates {
    fn prepare_approval(&self, request: ApprovalRequest) -> Result<GateFuture<ApprovalReply>> {
        // Insert must happen before the future is returned: the reactor
        // appends the observable event right after prepare, and an
        // immediately-resolving client must find the entry.
        prepare(
            &self.approvals,
            request.approval_id,
            "approval was cancelled before resolution",
        )
    }

    fn prepare_user_input(&self, request: UserInputRequest) -> Result<GateFuture<String>> {
        prepare(
            &self.inputs,
            request.request_id,
            "user input was cancelled before resolution",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_agent::ToolCallRequest;
    use std::time::Duration;

    fn call() -> ToolCallRequest {
        ToolCallRequest::new("call-1", "bash", serde_json::json!({}))
    }

    fn approval(id: &str) -> ApprovalRequest {
        ApprovalRequest::new(id, call(), "run command")
    }

    fn user_input(id: &str) -> UserInputRequest {
        UserInputRequest {
            request_id: id.into(),
            call: call(),
            prompt: "which branch?".into(),
        }
    }

    #[tokio::test]
    async fn resolving_before_await_reaches_the_waiter() {
        // No spawn/yield: resolve happens between prepare and first poll.
        let gates = InteractiveGates::new();
        let future = gates.prepare_approval(approval("approval-1")).unwrap();
        assert!(gates.resolve_approval("approval-1", true));
        assert!(future.await.unwrap().approved());
    }

    #[tokio::test]
    async fn resolving_denial_before_await_reaches_the_waiter() {
        let gates = InteractiveGates::new();
        let future = gates.prepare_approval(approval("approval-deny")).unwrap();
        assert!(gates.resolve_approval("approval-deny", false));
        let reply = future.await.unwrap();
        assert!(!reply.approved());
        assert_eq!(reply.reason(), "rejected by the user");
    }

    #[tokio::test]
    async fn dropping_the_future_without_awaiting_cleans_up() {
        let gates = InteractiveGates::new();
        drop(gates.prepare_approval(approval("approval-drop")).unwrap());
        assert_eq!(gates.pending_approvals(), 0);
        assert!(!gates.resolve_approval("approval-drop", true));
    }

    #[tokio::test]
    async fn stale_waiter_drop_keeps_the_reregistered_id() {
        // ABA: old registration resolved, id re-registered, old future then
        // dropped. The generation guard must not remove the new entry.
        let gates = InteractiveGates::new();
        let old = gates.prepare_approval(approval("approval-aba")).unwrap();
        assert!(gates.resolve_approval("approval-aba", true)); // removes old entry
        let new = gates.prepare_approval(approval("approval-aba")).unwrap();
        assert_eq!(gates.pending_approvals(), 1);

        drop(old); // stale guard fires: must keep the new registration
        assert_eq!(gates.pending_approval_ids(), vec!["approval-aba"]);

        assert!(gates.resolve_approval("approval-aba", false));
        let reply = new.await.unwrap();
        assert!(!reply.approved());
    }

    #[tokio::test]
    async fn stale_user_input_drop_keeps_the_reregistered_id() {
        // Same ABA shape for user inputs: the generation guard is shared.
        let gates = InteractiveGates::new();
        let old = gates.prepare_user_input(user_input("ask-aba")).unwrap();
        assert!(gates.resolve_user_input("ask-aba", "first".into()));
        let new = gates.prepare_user_input(user_input("ask-aba")).unwrap();
        assert_eq!(gates.pending_user_input_ids(), vec!["ask-aba"]);

        drop(old);
        assert_eq!(gates.pending_user_input_ids(), vec!["ask-aba"]);

        assert!(gates.resolve_user_input("ask-aba", "second".into()));
        assert_eq!(new.await.unwrap(), "second");
    }

    #[tokio::test]
    async fn approval_resolves_exactly_once() {
        let gates = InteractiveGates::new();
        let future = gates.prepare_approval(approval("approval-once")).unwrap();
        assert!(gates.resolve_approval("approval-once", true));
        assert!(!gates.resolve_approval("approval-once", false)); // entry gone
        assert!(future.await.unwrap().approved());
    }

    #[tokio::test]
    async fn user_input_resolves_exactly_once() {
        let gates = InteractiveGates::new();
        let future = gates.prepare_user_input(user_input("ask-once")).unwrap();
        assert!(gates.resolve_user_input("ask-once", "first".into()));
        assert!(!gates.resolve_user_input("ask-once", "second".into()));
        assert_eq!(future.await.unwrap(), "first");
    }

    #[tokio::test]
    async fn concurrent_resolves_deliver_exactly_one_reply() {
        // Deterministic concurrency: two racing resolvers, one barrier.
        let gates = Arc::new(InteractiveGates::new());
        let future = gates.prepare_approval(approval("approval-race")).unwrap();
        assert_eq!(gates.pending_approval_ids(), vec!["approval-race"]);

        let mut resolvers = tokio::task::JoinSet::new();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        for approved in [true, false] {
            let gates = Arc::clone(&gates);
            let barrier = Arc::clone(&barrier);
            resolvers.spawn(async move {
                barrier.wait().await;
                let resolved = gates.resolve_approval("approval-race", approved);
                (approved, resolved)
            });
        }
        // `join_next` returns tasks in completion order, so each result must
        // carry the `approved` it attempted alongside its `resolved` outcome.
        let mut results: Vec<(bool, bool)> = Vec::with_capacity(2);
        for _ in 0..2 {
            results.push(resolvers.join_next().await.unwrap().unwrap());
        }
        // Exactly one HTTP resolver wins (204); the loser sees 404 semantics.
        let winners: Vec<bool> = results
            .iter()
            .filter(|(_, resolved)| *resolved)
            .map(|(approved, _)| *approved)
            .collect();
        assert_eq!(winners.len(), 1);
        let winner_approved = winners[0];

        // The future observes exactly the winner's decision and converges.
        let reply = future.await.unwrap();
        assert_eq!(reply.approved(), winner_approved);
        // And the id is fully consumed: nothing left pending.
        assert_eq!(gates.pending_approval_ids(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn aborted_approval_task_cleans_up() {
        // Real cancellation: a spawned waiter is aborted mid-wait.
        let gates = Arc::new(InteractiveGates::new());
        let waiting = Arc::clone(&gates);
        let task = tokio::spawn(async move {
            waiting
                .prepare_approval(approval("approval-abort"))
                .unwrap()
                .await
        });
        // Wait until the task is parked on the reply channel, then abort it.
        tokio::time::timeout(Duration::from_secs(1), async {
            while gates.pending_approvals() == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("task registered and parked");

        task.abort();
        let result = task.await.unwrap_err();
        assert!(result.is_cancelled());
        assert_eq!(gates.pending_approvals(), 0);
        assert!(!gates.resolve_approval("approval-abort", true));
    }

    #[tokio::test]
    async fn aborted_user_input_task_cleans_up() {
        let gates = Arc::new(InteractiveGates::new());
        let waiting = Arc::clone(&gates);
        let task = tokio::spawn(async move {
            waiting
                .prepare_user_input(user_input("ask-abort"))
                .unwrap()
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while gates.pending_user_input_ids().is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("task registered and parked");

        task.abort();
        let result = task.await.unwrap_err();
        assert!(result.is_cancelled());
        assert!(gates.pending_user_input_ids().is_empty());
        assert!(!gates.resolve_user_input("ask-abort", "x".into()));
    }

    #[tokio::test]
    async fn duplicate_pending_id_is_conflict() {
        let gates = InteractiveGates::new();
        let first = gates.prepare_approval(approval("approval-dup")).unwrap();
        assert!(gates.prepare_approval(approval("approval-dup")).is_err());
        drop(first);
        // After cleanup a fresh registration succeeds again.
        assert!(gates.prepare_approval(approval("approval-dup")).is_ok());
    }

    #[tokio::test]
    async fn user_input_resolves_immediately_after_prepare() {
        let gates = InteractiveGates::new();
        let future = gates.prepare_user_input(user_input("ask-1")).unwrap();
        assert!(gates.resolve_user_input("ask-1", "main".into()));
        assert_eq!(future.await.unwrap(), "main");
    }

    #[tokio::test]
    async fn dropping_a_user_input_future_cleans_up() {
        let gates = InteractiveGates::new();
        drop(gates.prepare_user_input(user_input("ask-drop")).unwrap());
        assert_eq!(gates.pending_user_input_ids(), Vec::<String>::new());
        assert!(!gates.resolve_user_input("ask-drop", "x".into()));
    }
}
