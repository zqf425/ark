//! Shared server state and the seam the composition root fills in (§7.9 / §5.3).
//!
//! `ark-server` knows *ports*, never concrete providers: the turn orchestration,
//! the model client and the tool wiring are injected by `arkd`. That is what lets
//! the HTTP layer be tested against fakes and keeps `if plugin_id == ...` out of
//! the boundary entirely.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ark_agent::{CancelToken, SteerQueue};
use ark_common::{Result, ThreadId, TurnId, Usage};
use ark_config::{ConfigStore, TuningConfig};
use ark_contract::{RuntimeEvent, TurnSummary, UsageResponse};
use ark_obs::UsageRecorder;
use ark_ports::{
    AttachmentStore, BaseServices, SessionStore, SubagentRegistry, SubagentRunner, ToolRegistryPort,
};
use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, Mutex as AsyncMutex};

use crate::gates::InteractiveGates;
use crate::service::continuation::ContinuationRegistry;

/// Durable abort reason used for process-owned shutdown cancellation.
pub const SHUTDOWN_CANCEL_REASON: &str = "runtime shutting down";

/// Terminal outcome a turn run reported (design §7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    Completed,
    /// Completed normally and acknowledged the delivered inbox, but more durable
    /// completions remain for another bounded continuation batch.
    CompletedWithPendingCompletions,
    Failed {
        message: String,
        /// `true` when the failure came from the model request itself. The
        /// auto-continuation circuit breaker counts consecutive occurrences;
        /// other failures (tool, policy, panic) do not trip it.
        model_request_failed: bool,
    },
    Aborted(String),
}

/// Source that started a parent turn.
pub enum TurnSource {
    /// Explicit user input, persisted as a user message.
    User(String),
    /// System-origin continuation consuming the durable child-completion inbox.
    AutoContinuation,
}

/// One unit of work handed to the turn runner.
pub struct TurnJob {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub source: TurnSource,
    pub cancel: CancelToken,
    /// The active turn's bounded correction queue. The registry creates it
    /// before the worker starts, so `/steer` and the reactor share one handle.
    pub steering: Arc<SteerQueue>,
}

/// The handles created atomically with a newly accepted turn.
pub struct TurnRegistration {
    pub summary: TurnSummary,
    pub steering: Arc<SteerQueue>,
}

/// Why an active turn could not accept a correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerError {
    NotFound,
    Finished,
    QueueFull,
}

/// The single orchestration seam: `arkd` runs one turn through the reactor.
#[async_trait::async_trait]
pub trait TurnRunner: Send + Sync {
    /// Run the turn to a terminal state. `Err` means the turn could not start.
    async fn run_turn(&self, job: TurnJob) -> Result<TurnEnd>;
}

/// Live turn bookkeeping (state + the cancel handle used by `/interrupt`).
pub struct TurnRegistry {
    inner: Mutex<HashMap<TurnId, TurnSlot>>,
    latest_by_thread: Mutex<HashMap<ThreadId, TurnId>>,
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
            inner: Mutex::new(HashMap::new()),
            latest_by_thread: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<TurnId, TurnSlot>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
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
        let busy = guard
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
        guard.insert(
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

    /// The in-flight turn of a thread, if any.
    pub fn active_for(&self, thread_id: ThreadId) -> Option<TurnSummary> {
        self.lock()
            .values()
            .find(|slot| slot.summary.thread_id == thread_id && slot.summary.ended_at.is_none())
            .map(|slot| slot.summary.clone())
    }

    pub fn get(&self, turn_id: TurnId) -> Option<TurnSummary> {
        self.lock().get(&turn_id).map(|slot| slot.summary.clone())
    }

    /// The newest process-local lifecycle epoch registered for this thread.
    pub fn latest_for(&self, thread_id: ThreadId) -> Option<TurnId> {
        self.latest_by_thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&thread_id)
            .copied()
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
            .get(&turn_id)
            .filter(|slot| slot.summary.ended_at.is_none())
            .map(|slot| Arc::clone(&slot.steering))
    }

    /// Queue a correction for the exact active turn and report saturation
    /// instead of dropping it silently.
    pub fn enqueue_steering(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        text: &str,
    ) -> std::result::Result<TurnSummary, SteerError> {
        let guard = self.lock();
        let slot = guard.get(&turn_id).ok_or(SteerError::NotFound)?;
        if slot.summary.thread_id != thread_id {
            return Err(SteerError::NotFound);
        }
        if slot.summary.ended_at.is_some() {
            return Err(SteerError::Finished);
        }
        if !slot.steering.push(text) {
            return Err(SteerError::QueueFull);
        }
        Ok(slot.summary.clone())
    }

    /// Trip the turn's cancel token, if the turn is still known.
    pub fn cancel(&self, turn_id: TurnId) -> Option<TurnSummary> {
        let mut guard = self.lock();
        let slot = guard.get_mut(&turn_id)?;
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
        let slot = guard.get_mut(&turn_id)?;
        if slot.summary.thread_id != thread_id || slot.summary.ended_at.is_some() {
            return None;
        }
        slot.cancel.cancel();
        slot.summary.state = "aborting".to_string();
        Some(slot.summary.clone())
    }

    /// Record the terminal state reported by the runner.
    pub fn finish(&self, turn_id: TurnId, end: &TurnEnd) {
        let mut guard = self.lock();
        let Some(slot) = guard.get_mut(&turn_id) else {
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
        slot.summary.ended_at = Some(Utc::now());
    }

    /// Cancel every in-flight turn with the ordinary interrupt reason.
    pub fn cancel_all(&self) {
        self.cancel_all_with_reason("turn interrupted");
    }

    /// Cancel every in-flight turn with one durable process-owned reason.
    pub fn cancel_all_with_reason(&self, reason: &str) {
        for slot in self.lock().values_mut() {
            slot.cancel.cancel_with_reason(reason);
            if slot.summary.ended_at.is_none() {
                slot.summary.state = "aborting".to_string();
            }
        }
    }

    pub fn active(&self) -> usize {
        self.lock()
            .values()
            .filter(|slot| slot.summary.ended_at.is_none())
            .count()
    }
}

/// Everything a route needs. Constructed once by `arkd` and cloned per request.
#[derive(Clone)]
pub struct AppState {
    /// Immutable attachment storage injected by the composition root.
    pub attachments: Option<Arc<dyn AttachmentStore>>,
    pub store: Arc<dyn SessionStore>,
    pub tools: Arc<dyn ToolRegistryPort>,
    pub config: Arc<ConfigStore>,
    pub usage: Arc<UsageRecorder>,
    pub turns: Arc<TurnRegistry>,
    pub continuations: Arc<ContinuationRegistry>,
    /// Set before shutdown cancellation so late child completions cannot schedule
    /// fresh parent work while the listener is draining.
    shutting_down: Arc<AtomicBool>,
    /// The one-shot gate rendezvous shared with the reactor injected by arkd.
    pub gates: Arc<InteractiveGates>,
    pub runner: Arc<dyn TurnRunner>,
    pub started_at: DateTime<Utc>,
    pub version: String,
    pub data_dir: PathBuf,
    /// Sub-agent surface (design §7.8). Absent in a build without delegation,
    /// which makes the `child-runs` routes answer a structured `unavailable`
    /// instead of panicking on an unwrap.
    pub subagents: SubagentServices,
    /// Bearer token required on every non-`/health` route; `None` disables auth.
    pub auth_token: Option<String>,
    /// Platform services (providers / skills / workspace / MCP servers).
    ///
    /// Held as port traits so this crate never depends on the concrete stores;
    /// a missing service makes its routes answer a structured "unavailable".
    pub base: BaseServices,
    /// Serializes a workspace switch with parent-turn registration. A lease is
    /// captured immediately after registration, so neither operation can slip
    /// through the other's zero-active check.
    pub workspace_gate: Arc<AsyncMutex<()>>,
    shutdown: Arc<broadcast::Sender<&'static str>>,
    turns_completed: Arc<AtomicU64>,
}

/// The two halves of the sub-agent surface.
///
/// Spawning a child ([`SubagentRunner`]) and managing one
/// ([`SubagentRegistry`]) are deliberately separate traits, so the boundary never
/// gains the ability to start a run just because it can list one.
#[derive(Clone, Default)]
pub struct SubagentServices {
    pub runner: Option<Arc<dyn SubagentRunner>>,
    pub registry: Option<Arc<dyn SubagentRegistry>>,
}

impl AppState {
    /// Assemble the state. `shutdown` is the sender `serve` waits on.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<dyn SessionStore>,
        tools: Arc<dyn ToolRegistryPort>,
        config: Arc<ConfigStore>,
        usage: Arc<UsageRecorder>,
        runner: Arc<dyn TurnRunner>,
        data_dir: PathBuf,
        version: impl Into<String>,
    ) -> Self {
        let (shutdown, _) = broadcast::channel(4);
        Self {
            attachments: None,
            store,
            tools,
            config,
            usage,
            turns: Arc::new(TurnRegistry::new()),
            continuations: Arc::new(ContinuationRegistry::new()),
            shutting_down: Arc::new(AtomicBool::new(false)),
            gates: Arc::new(InteractiveGates::new()),
            runner,
            started_at: Utc::now(),
            version: version.into(),
            data_dir,
            subagents: SubagentServices::default(),
            auth_token: None,
            base: BaseServices::default(),
            workspace_gate: Arc::new(AsyncMutex::new(())),
            shutdown: Arc::new(shutdown),
            turns_completed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Attach the immutable attachment-store implementation.
    pub fn with_attachments(mut self, attachments: Arc<dyn AttachmentStore>) -> Self {
        self.attachments = Some(attachments);
        self
    }

    /// Attach the platform services (design §7.5.4).
    ///
    /// Only `arkd` calls this, and it passes every concrete store it built, so
    /// the boundary keeps talking to ports while the composition root keeps
    /// being the one place that knows implementations.
    pub fn with_base_services(mut self, base: BaseServices) -> Self {
        self.base = base;
        self
    }

    /// Attach the sub-agent services (design §7.8).
    ///
    /// Only `arkd` calls this, and it passes the same runtime object behind both
    /// traits, so listing and spawning always agree about which children exist.
    pub fn with_subagents(
        mut self,
        runner: Arc<dyn SubagentRunner>,
        registry: Arc<dyn SubagentRegistry>,
    ) -> Self {
        self.subagents = SubagentServices {
            runner: Some(runner),
            registry: Some(registry),
        };
        self
    }

    /// Attach the same interactive gate broker the injected turn runner uses.
    pub fn with_gates(mut self, gates: Arc<InteractiveGates>) -> Self {
        self.gates = gates;
        self
    }

    /// Require a bearer token on every route except `/health`.
    pub fn with_auth_token(mut self, token: Option<String>) -> Self {
        self.auth_token = token.filter(|value| !value.trim().is_empty());
        self
    }

    /// Enter shutdown before cancelling work, rejecting late continuation wakes.
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        self.continuations
            .cancel_all_with_reason(SHUTDOWN_CANCEL_REASON);
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    /// Signal graceful shutdown; returns how many subscribers were told.
    pub fn request_shutdown(&self, reason: &'static str) -> usize {
        self.begin_shutdown();
        self.shutdown.send(reason).unwrap_or(0)
    }

    /// Subscribe to shutdown, for `serve`'s graceful path.
    pub fn shutdown_receiver(&self) -> broadcast::Receiver<&'static str> {
        self.shutdown.subscribe()
    }

    /// Current tuning snapshot (read on every request, never cached here).
    pub fn tuning(&self) -> TuningConfig {
        self.config.snapshot().tuning.clone()
    }

    pub fn note_turn_completed(&self) -> u64 {
        self.turns_completed.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn turns_completed(&self) -> u64 {
        self.turns_completed.load(Ordering::Relaxed)
    }

    /// `GET /v1/usage` payload, built only from real counters.
    pub fn usage_response(&self) -> UsageResponse {
        UsageResponse {
            turns: self.turns_completed(),
            usage: self.usage.usage(),
        }
    }

    /// Fold one provider-reported sample into the runtime total.
    pub fn record_usage(&self, usage: &Usage) {
        self.usage.record(usage);
    }

    /// Make sure a turn the supervisor closed still has a complete lifecycle.
    ///
    /// The reactor writes `turn_started` before its first model round and its own
    /// terminal event on every path it controls. The supervisor closes the paths
    /// the reactor never reaches: a panic, a dropped task, or a runner error
    /// before `prepare()`. Left alone those turns would either end the log at
    /// `turn_started` (a reconnect replays a turn that can never finish) or emit
    /// a terminal record for a turn the log never shows starting — an audit trail
    /// that rewind, fork and replay cannot reconcile (design §7.1 / §7.9).
    ///
    /// Replay-then-append keeps this idempotent: a turn the reactor already
    /// closed, or one that already has a start record, is never duplicated.
    pub async fn record_turn_terminal(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        auto_continuation: bool,
        end: &TurnEnd,
    ) {
        // Narrow query: only turn and gate lifecycle entries are needed, so a
        // long transcript is never cloned just to close one turn.
        let events = match self.store.replay_interaction_events(thread_id).await {
            Ok(events) => events,
            // An unreadable log must not be "fixed" by inventing records; the
            // warning in the supervisor already reports it.
            Err(_) => return,
        };
        if events.iter().any(|event| closes_turn(event, turn_id)) {
            return;
        }
        if !events.iter().any(|event| starts_turn(event, turn_id)) {
            // The runner failed before the reactor journaled its start. Record it
            // now, so the terminal record below always has a counterpart and the
            // automatic delivery accounting still sees this attempt.
            let started = RuntimeEvent::TurnStarted {
                seq: 0,
                thread_id,
                turn_id,
                timestamp: Utc::now(),
                auto_continuation,
            };
            if let Err(err) = self.store.append_event(thread_id, started).await {
                tracing::warn!(
                    turn = %turn_id,
                    error = %err.message,
                    "could not persist the synthesized turn start"
                );
                return;
            }
        }
        let event = terminal_event(thread_id, turn_id, end);
        if let Err(err) = self.store.append_event(thread_id, event).await {
            tracing::warn!(
                turn = %turn_id,
                error = %err.message,
                "could not persist a synthesized terminal event"
            );
        }
    }

    /// Abort every live child run (shutdown path, design §7.9).
    ///
    /// Returns how many were asked to stop. Children converge to their own
    /// terminal records through the delegation supervisor, which is why shutdown
    /// only has to trip the handles and wait.
    pub fn abort_children(&self) -> usize {
        let Some(registry) = self.subagents.registry.as_ref() else {
            return 0;
        };
        let mut aborted = 0;
        for record in registry.list(None) {
            if record.is_active() && registry.abort(record.child_id) {
                aborted += 1;
            }
        }
        aborted
    }

    /// Children that still have not reached a terminal state.
    pub fn active_children(&self) -> usize {
        self.subagents
            .registry
            .as_ref()
            .map(|registry| {
                registry
                    .list(None)
                    .iter()
                    .filter(|record| record.is_active())
                    .count()
            })
            .unwrap_or(0)
    }
}

/// Whether `event` opens the lifecycle of `turn_id`.
fn starts_turn(event: &RuntimeEvent, turn_id: TurnId) -> bool {
    matches!(event, RuntimeEvent::TurnStarted { turn_id: id, .. } if *id == turn_id)
}

/// Whether `event` is the terminal record of `turn_id`.
fn closes_turn(event: &RuntimeEvent, turn_id: TurnId) -> bool {
    match event {
        RuntimeEvent::TurnCompleted { turn_id: id, .. }
        | RuntimeEvent::TurnFailed { turn_id: id, .. }
        | RuntimeEvent::TurnAborted { turn_id: id, .. } => *id == turn_id,
        _ => false,
    }
}

/// The terminal event a supervisor-synthesized close records.
fn terminal_event(thread_id: ThreadId, turn_id: TurnId, end: &TurnEnd) -> RuntimeEvent {
    match end {
        TurnEnd::Completed | TurnEnd::CompletedWithPendingCompletions => {
            RuntimeEvent::TurnCompleted {
                seq: 0,
                thread_id,
                turn_id,
                timestamp: Utc::now(),
            }
        }
        TurnEnd::Failed {
            message,
            model_request_failed,
        } => RuntimeEvent::TurnFailed {
            seq: 0,
            thread_id,
            turn_id,
            timestamp: Utc::now(),
            message: message.clone(),
            model_request_failed: *model_request_failed,
        },
        TurnEnd::Aborted(reason) => RuntimeEvent::TurnAborted {
            seq: 0,
            thread_id,
            turn_id,
            timestamp: Utc::now(),
            reason: reason.clone(),
        },
    }
}

/// Adapter used by `arkd` so the model client reports usage without the server
/// knowing which client it is.
pub struct SharedUsage(pub Arc<UsageRecorder>);

impl SharedUsage {
    pub fn record(&self, usage: &Usage) {
        self.0.record(usage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_finish_moves_the_state() {
        let registry = TurnRegistry::new();
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let summary = registry
            .try_register(thread, turn, CancelToken::new())
            .expect("first turn on an idle thread must be accepted");
        assert_eq!(summary.summary.state, "running");
        registry.finish(turn, &TurnEnd::Completed);
        let done = registry.get(turn).unwrap();
        assert_eq!(done.state, "completed");
        assert!(done.ended_at.is_some());
        assert_eq!(registry.active(), 0);
    }

    #[test]
    fn a_second_turn_on_a_busy_thread_is_rejected() {
        // Mirrors `conflictPolicy: 'reject'`: the API answers 409 instead of
        // interleaving two model loops over one event log.
        let registry = TurnRegistry::new();
        let thread = ThreadId::new();
        let first = TurnId::new();
        assert!(registry
            .try_register(thread, first, CancelToken::new())
            .is_some());
        assert!(registry
            .try_register(thread, TurnId::new(), CancelToken::new())
            .is_none());
        assert_eq!(registry.active_for(thread).unwrap().turn_id, first);
    }

    #[test]
    fn a_busy_thread_accepts_a_new_turn_once_the_previous_one_ends() {
        let registry = TurnRegistry::new();
        let thread = ThreadId::new();
        let first = TurnId::new();
        registry.try_register(thread, first, CancelToken::new());
        registry.finish(first, &TurnEnd::Completed);
        assert!(registry
            .try_register(thread, TurnId::new(), CancelToken::new())
            .is_some());
    }

    #[test]
    fn different_threads_run_concurrently() {
        let registry = TurnRegistry::new();
        let a = ThreadId::new();
        let b = ThreadId::new();
        assert!(registry
            .try_register(a, TurnId::new(), CancelToken::new())
            .is_some());
        assert!(registry
            .try_register(b, TurnId::new(), CancelToken::new())
            .is_some());
        assert_eq!(registry.active(), 2);
    }

    #[test]
    fn cancel_trips_the_token_and_marks_aborting() {
        let registry = TurnRegistry::new();
        let token = CancelToken::new();
        let turn = TurnId::new();
        registry.try_register(ThreadId::new(), turn, token.clone());
        let summary = registry.cancel(turn).unwrap();
        assert_eq!(summary.state, "aborting");
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_active_checks_owner_and_terminal_state_atomically() {
        let registry = TurnRegistry::new();
        let owner = ThreadId::new();
        let other = ThreadId::new();
        let turn = TurnId::new();
        let token = CancelToken::new();
        registry.try_register(owner, turn, token.clone());

        assert!(registry.cancel_active(other, turn).is_none());
        assert!(!token.is_cancelled());
        let cancelled = registry
            .cancel_active(owner, turn)
            .expect("owned active turn");
        assert_eq!(cancelled.state, "aborting");
        assert!(token.is_cancelled());

        registry.finish(turn, &TurnEnd::Completed);
        assert!(registry.cancel_active(owner, turn).is_none());
        assert_eq!(registry.get(turn).unwrap().state, "completed");
    }

    #[test]
    fn failed_turns_keep_their_message() {
        let registry = TurnRegistry::new();
        let turn = TurnId::new();
        registry.try_register(ThreadId::new(), turn, CancelToken::new());
        registry.finish(
            turn,
            &TurnEnd::Failed {
                message: "boom".into(),
                model_request_failed: false,
            },
        );
        let summary = registry.get(turn).unwrap();
        assert_eq!(summary.state, "failed");
        assert_eq!(summary.error.as_deref(), Some("boom"));
    }

    #[test]
    fn cancel_all_trips_every_live_turn() {
        let registry = TurnRegistry::new();
        let first = CancelToken::new();
        let second = CancelToken::new();
        registry.try_register(ThreadId::new(), TurnId::new(), first.clone());
        registry.try_register(ThreadId::new(), TurnId::new(), second.clone());
        registry.cancel_all_with_reason(SHUTDOWN_CANCEL_REASON);
        assert!(first.is_cancelled() && second.is_cancelled());
        assert_eq!(first.reason().as_deref(), Some(SHUTDOWN_CANCEL_REASON));
        assert_eq!(second.reason().as_deref(), Some(SHUTDOWN_CANCEL_REASON));
    }

    #[test]
    fn newest_turn_marker_advances_per_thread() {
        let registry = TurnRegistry::new();
        let thread = ThreadId::new();
        let first = TurnId::new();
        let second = TurnId::new();
        registry.try_register(thread, first, CancelToken::new());
        registry.finish(first, &TurnEnd::Completed);
        registry.try_register(thread, second, CancelToken::new());

        assert!(!registry.is_latest_for(thread, first));
        assert!(registry.is_latest_for(thread, second));
        assert!(!registry.is_latest_for(ThreadId::new(), second));
    }

    #[test]
    fn active_turn_steering_reaches_its_reactor_queue() {
        let registry = TurnRegistry::new();
        let thread = ThreadId::new();
        let turn = TurnId::new();
        registry.try_register(thread, turn, CancelToken::new());
        let queue = registry
            .steering_for(turn)
            .expect("a running turn owns a steering queue");

        registry
            .enqueue_steering(thread, turn, "focus on the failing test")
            .expect("an active turn accepts steering");

        assert_eq!(queue.drain(), vec!["focus on the failing test"]);
    }

    #[test]
    fn unknown_turn_lookups_are_none() {
        let registry = TurnRegistry::new();
        assert!(registry.get(TurnId::new()).is_none());
        assert!(registry.cancel(TurnId::new()).is_none());
    }
}
