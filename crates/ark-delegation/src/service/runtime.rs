//! Child-run orchestration (design §7.8).
//!
//! The runtime owns everything around a child run that the reactor must not know
//! about: the concurrency budget, the persisted record, abort/steer handles, and
//! the `<parent> -> children` activity events.
//!
//! It never contains a decision loop. `ChildTurnExecutor` (implemented by `arkd`)
//! runs the *same* reactor against the child session; this crate only decides
//! whether that may start, records what happened, and hands out control handles.
//!
//! # Lifecycle guarantees
//!
//! Every run reaches a terminal record, and every drain of the budget is
//! released. Two mechanisms enforce that, because a `Drop` impl cannot await:
//!
//! * [`RunSlot`] is a **synchronous** guard that always releases the control
//!   handle and the `active` counter, including on panic unwinding;
//! * a run executes on its **own task** ([`DelegationRuntime::delegate`],
//!   [`DelegationRuntime::run_child`]), so a cancelled parent turn cannot abandon
//!   a half-run child, and a panicking executor is observed through its
//!   `JoinHandle` and recorded as a failure.
//!
//! Anything that still escapes (a process killed mid-run) is converged by
//! [`DelegationRuntime::hydrate`] at the next start, which is the only correct
//! place to repair state that outlives the process.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ark_common::{ArkError, ArkErrorCode, ChildId, Result, ThreadId};
use ark_contract::RuntimeEvent;
use ark_ports::{
    CancelHandle, ChildRunRecord, EventSink, SessionStore, SteerHandle, SubagentOutcome,
    SubagentPolicy, SubagentPolicySource, SubagentRegistry, SubagentRequest, SubagentRunner,
};
use chrono::Utc;
use tokio::sync::{broadcast, oneshot, Notify};

use crate::domain::{
    acquire_error, finished_record, ChildTurnContext, ChildTurnReport, SlotGate, StopReason,
};

/// How often a queued run re-checks its cancel token.
///
/// Abort is a human-paced action, so 25 ms worst-case latency is invisible, and
/// polling keeps the wait path allocation-free without a second wakeup channel.
const QUEUE_POLL: Duration = Duration::from_millis(25);

/// Runs one child turn. Implemented by `arkd`, because only the composition root
/// may wire a model client, a tool runner and a session writer together.
#[async_trait::async_trait]
pub trait ChildTurnExecutor: Send + Sync {
    /// Run the child's turn to a terminal state.
    ///
    /// Returning `Err` means the turn could not even start; the runtime records
    /// that as a failed run rather than propagating it to the parent.
    async fn run_child_turn(&self, ctx: ChildTurnContext) -> Result<ChildTurnReport>;
}

/// Handle for one child, kept so the boundary can abort or steer it.
///
/// A handle exists from the moment a run is accepted — **before** it owns a slot
/// — so a child waiting behind a saturated budget is still abortable.
pub struct RunningChild {
    pub child_id: ChildId,
    pub parent_thread_id: ThreadId,
    cancel: Arc<dyn CancelHandle>,
    steer: Arc<dyn SteerHandle>,
}

impl RunningChild {
    /// Trip the child's cancel token. The reactor stops at its next safe boundary.
    pub fn abort(&self) {
        self.cancel.cancel();
    }

    /// Queue a correction. `false` when the queue is full.
    pub fn steer(&self, text: &str) -> bool {
        self.steer.push(text)
    }

    /// Whether the run has been asked to stop.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl std::fmt::Debug for RunningChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningChild")
            .field("child_id", &self.child_id)
            .field("parent_thread_id", &self.parent_thread_id)
            .finish_non_exhaustive()
    }
}

/// A pre-registered record, produced by [`DelegationRuntime::prepare`].
///
/// Separating preparation from execution is what lets `delegate_task` return a
/// `queued` child id immediately for a background run, while the foreground path
/// awaits the same prepared run.
#[derive(Debug, Clone)]
pub struct PreparedChild {
    pub record: ChildRunRecord,
    pub ctx: ChildTurnContext,
    /// Slot gate inputs captured at spawn time, so a settings change mid-wait
    /// does not silently retune an already-accepted run.
    pub policy: SubagentPolicy,
}

/// Mutable runtime state, behind one `Arc` so a run can be moved onto its own
/// task while the boundary keeps calling `list`/`abort`/`steer`.
struct RuntimeCore {
    store: Arc<dyn SessionStore>,
    sink: Arc<dyn EventSink>,
    policy: Arc<dyn SubagentPolicySource>,
    executor: Arc<dyn ChildTurnExecutor>,
    slot_gate: Arc<SlotGate>,
    /// Control handles for every accepted child, **including queued ones**.
    handles: Arc<Mutex<HashMap<ChildId, Arc<RunningChild>>>>,
    /// Last known record per child, so `get`/`list` do not re-read the log on
    /// every HTTP call while still treating the log as the system of record.
    known: Arc<Mutex<HashMap<ChildId, ChildRunRecord>>>,
    active: Arc<AtomicU32>,
    /// Runs created by [`DelegationRuntime::prepare_child`] that no task has
    /// claimed yet. Keeping them here is what makes the prepare/run split safe:
    /// `run_child` *removes* its entry, so two tasks can never execute the same
    /// child twice.
    prepared: Mutex<HashMap<ChildId, PreparedChild>>,
    /// Children destroyed through `DELETE /v1/child-runs/{id}`.
    ///
    /// A run that is still finishing must not re-create a record the user just
    /// deleted, so `relabel`/`finish` consult this before every append.
    deleted: Arc<Mutex<HashSet<ChildId>>>,
    /// Notified whenever a run reaches a terminal state (used by tests and by a
    /// future "wait for all children" shutdown path).
    settled: Arc<Notify>,
    /// Best-effort process-local wakeup emitted only after the durable
    /// `child_completed` append succeeds. Durable inbox events remain the source
    /// of truth; this channel only avoids polling them.
    completions: broadcast::Sender<ThreadId>,
}

/// Sub-agent runtime: budget, records, handles, activity events.
#[derive(Clone)]
pub struct DelegationRuntime {
    core: Arc<RuntimeCore>,
}

impl std::fmt::Debug for DelegationRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegationRuntime")
            .field("handles", &self.core.lock_handles().len())
            .field("active", &self.core.active.load(Ordering::Relaxed))
            .field("queued", &self.core.slot_gate.queued())
            .finish_non_exhaustive()
    }
}

impl DelegationRuntime {
    pub fn new(
        store: Arc<dyn SessionStore>,
        sink: Arc<dyn EventSink>,
        policy: Arc<dyn SubagentPolicySource>,
        executor: Arc<dyn ChildTurnExecutor>,
    ) -> Self {
        let (completions, _) = broadcast::channel(256);
        Self {
            core: Arc::new(RuntimeCore {
                store,
                sink,
                policy,
                executor,
                slot_gate: Arc::new(SlotGate::new()),
                handles: Arc::new(Mutex::new(HashMap::new())),
                known: Arc::new(Mutex::new(HashMap::new())),
                active: Arc::new(AtomicU32::new(0)),
                prepared: Mutex::new(HashMap::new()),
                deleted: Arc::new(Mutex::new(HashSet::new())),
                settled: Arc::new(Notify::new()),
                completions,
            }),
        }
    }

    /// Notified when any child reaches a terminal state.
    pub fn settled(&self) -> Arc<Notify> {
        Arc::clone(&self.core.settled)
    }

    /// Subscribe to successfully persisted parent completion notifications.
    pub fn subscribe_completions(&self) -> broadcast::Receiver<ThreadId> {
        self.core.completions.subscribe()
    }

    /// The live policy, read on every spawn so a Settings change applies to the
    /// next child without a restart.
    pub fn policy(&self) -> SubagentPolicy {
        self.core.policy.policy()
    }

    /// Load every authoritative child-run record into the in-memory registry.
    ///
    /// `children/runs.jsonl` is the system of record (design §7.4.1), but the
    /// registry the HTTP layer queries is memory-resident. Without this step a
    /// restart makes every historical child disappear from `list`, and ownership
    /// checks answer 404 for runs that exist on disk.
    ///
    /// Runs that were still `queued`/`running` when the previous process died are
    /// converged to a terminal state here. That is the authoritative safety net
    /// for a run whose future was dropped by a parent interrupt or a panic: it is
    /// exactly the `restart` case, and it is what keeps a resume from being
    /// refused forever.
    ///
    /// Returns how many distinct children were loaded.
    pub async fn hydrate(&self) -> Result<u32> {
        self.core.hydrate().await
    }

    /// Destroy one child run: refuse while active, then delete storage.
    pub async fn destroy_child(&self, parent: ThreadId, child: ChildId) -> Result<bool> {
        self.core.destroy_child(parent, child).await
    }

    /// Children holding a concurrency slot as seen by the gate (diagnostics).
    pub fn queued_count(&self) -> u32 {
        self.core.slot_gate.queued() as u32
    }

    /// Abort every known child (used by graceful shutdown).
    ///
    /// Queued children are included: they own a handle before they own a slot.
    pub fn abort_all(&self) {
        let handles: Vec<Arc<RunningChild>> = self.core.lock_handles().values().cloned().collect();
        for handle in handles {
            handle.abort();
        }
    }

    /// Create the child session and its `queued` record.
    pub async fn prepare(&self, request: SubagentRequest) -> Result<PreparedChild> {
        self.core.prepare(request).await
    }

    /// Run a prepared child to a terminal state under a runtime-owned supervisor.
    pub async fn execute(&self, prepared: PreparedChild) -> Result<SubagentOutcome> {
        self.await_supervised(self.start_supervised(prepared)).await
    }

    /// Prepare a run and park it until someone calls [`Self::run_child`].
    ///
    /// Used by `POST /v1/child-runs/{id}/resume`: the request must answer with
    /// the queued record, while the run itself continues on a background task.
    /// Everything that can be decided now — policy, depth, unknown child,
    /// already completed, still active — is decided here and surfaces as an
    /// error, so the client never gets an accepted-but-doomed resume.
    pub async fn prepare_child(&self, request: SubagentRequest) -> Result<ChildId> {
        let prepared = self.core.prepare(request).await?;
        let child_id = prepared.record.child_id;
        self.core.lock_prepared().insert(child_id, prepared);
        Ok(child_id)
    }

    /// Claim a prepared run and execute it to a terminal state.
    ///
    /// `NotFound` when the id is not parked here, which is what stops a second
    /// task (a retried HTTP resume, a stray caller) from running the same child
    /// twice.
    pub async fn run_child(&self, child: ChildId) -> Result<SubagentOutcome> {
        let prepared = self.core.lock_prepared().remove(&child).ok_or_else(|| {
            ArkError::not_found(format!(
                "child run {child} is not awaiting execution; it was already claimed"
            ))
        })?;
        self.await_supervised(self.start_supervised(prepared)).await
    }

    /// Convenience for the foreground path: prepare then await the supervised run.
    ///
    /// Dropping this caller only drops the oneshot receiver. The supervisor and
    /// worker remain owned by Tokio and still persist a terminal record.
    pub async fn delegate(&self, request: SubagentRequest) -> Result<SubagentOutcome> {
        let prepared = self.core.prepare(request).await?;
        self.await_supervised(self.start_supervised(prepared)).await
    }

    /// Start a detached child and return only after its durable queued record and
    /// control handle both exist.
    pub async fn delegate_background(&self, request: SubagentRequest) -> Result<ChildId> {
        let prepared = self.core.prepare(request).await?;
        let child = prepared.record.child_id;
        drop(self.start_supervised(prepared));
        Ok(child)
    }

    /// Install the handle synchronously, then spawn an independent supervisor.
    ///
    /// The supervisor owns the worker JoinHandle, so a foreground caller being
    /// interrupted cannot also discard panic handling for the child.
    fn start_supervised(
        &self,
        mut prepared: PreparedChild,
    ) -> oneshot::Receiver<Result<SubagentOutcome>> {
        self.core.install_handle(&mut prepared);
        let record = prepared.record.clone();
        let child = record.child_id;
        let core = Arc::clone(&self.core);
        let worker_core = Arc::clone(&core);
        let worker = tokio::spawn(async move { worker_core.execute(prepared).await });
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = match worker.await {
                Ok(result) => result,
                Err(join) => {
                    // `RunSlot` removes this after slot acquisition; this covers
                    // a panic before the slot/guard exists as well.
                    core.lock_handles().remove(&child);
                    let message = if join.is_panic() {
                        "child executor panicked"
                    } else {
                        "child run was cancelled before it finished"
                    };
                    core.finish(record, ChildTurnReport::failed(message))
                        .await
                        .map(|finished| outcome_of(&finished))
                }
            };
            core.settled.notify_waiters();
            let _ = tx.send(result);
        });
        rx
    }

    async fn await_supervised(
        &self,
        rx: oneshot::Receiver<Result<SubagentOutcome>>,
    ) -> Result<SubagentOutcome> {
        rx.await.map_err(|_| {
            ArkError::internal("child supervisor stopped before reporting its outcome")
        })?
    }
}

mod core;

fn completion_notification_id(record: &ChildRunRecord) -> String {
    format!(
        "child:{}:resume:{}",
        record.child_id,
        record.resume_count.unwrap_or_default()
    )
}

fn completion_event(record: &ChildRunRecord, notification_id: String) -> RuntimeEvent {
    RuntimeEvent::ChildCompleted {
        seq: 0,
        thread_id: record.parent_thread_id,
        timestamp: record.ended_at.unwrap_or_else(Utc::now),
        notification_id,
        child_id: record.child_id,
        label: record.label.clone(),
        status: record.status.clone(),
        summary: record.summary.clone(),
        error: record.error.clone(),
        usage: record.usage.clone(),
        tool_invocations: record.tool_invocations,
    }
}

/// Synchronous cleanup guard for one running child.
///
/// A `Drop` impl cannot await, so this only releases the *in-memory* budget and
/// handle; the terminal record is written by [`RuntimeCore::execute`] on its
/// normal paths, by [`DelegationRuntime::spawn_run`] when the task panicked, or
/// by [`RuntimeCore::hydrate`] after a hard process death.
struct RunSlot {
    handles: Arc<Mutex<HashMap<ChildId, Arc<RunningChild>>>>,
    active: Arc<AtomicU32>,
    child: ChildId,
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        if let Ok(mut handles) = self.handles.lock() {
            handles.remove(&self.child);
        }
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

fn child_context(request: &SubagentRequest, child_id: ChildId, prompt: String) -> ChildTurnContext {
    ChildTurnContext {
        child_id,
        parent_thread_id: request.parent_thread_id,
        parent_turn_id: request.parent_turn_id,
        prompt,
        workspace: request.workspace.clone().map(std::path::PathBuf::from),
        model: request.model.clone(),
        provider_id: request.provider_id.clone(),
        autonomy: request.autonomy.clone(),
        cancel: Arc::new(NoCancel),
        steer: Arc::new(NoSteer),
    }
}

/// Resolve once the handle has been cancelled.
async fn wait_cancelled(cancel: Arc<dyn CancelHandle>) {
    loop {
        if cancel.is_cancelled() {
            return;
        }
        tokio::time::sleep(QUEUE_POLL).await;
    }
}

fn outcome_of(record: &ChildRunRecord) -> SubagentOutcome {
    SubagentOutcome {
        child_id: record.child_id,
        status: record.status.clone(),
        summary: record.summary.clone(),
        error: record.error.clone(),
        usage: record.usage.clone().unwrap_or_default(),
        tool_invocations: record.tool_invocations.unwrap_or(0),
        model: record.model.clone(),
        provider_id: record.provider_id.clone(),
    }
}

/// Default label from the first line of the prompt, so a run is identifiable even
/// when the caller passes no label.
fn default_label(prompt: &str) -> String {
    let first = prompt.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        "child run".to_string()
    } else {
        ark_common::text::truncate_utf8(first, 60).to_string()
    }
}

// ---------------------------------------------------------------------------
// Default handles
// ---------------------------------------------------------------------------

/// A child that cannot be aborted yet (before `execute` attaches a real token).
#[derive(Debug, Default)]
struct NoCancel;

impl CancelHandle for NoCancel {
    fn cancel(&self) {}

    fn is_cancelled(&self) -> bool {
        false
    }
}

/// A child with no steering queue yet.
#[derive(Debug, Default)]
struct NoSteer;

impl SteerHandle for NoSteer {
    fn push(&self, _text: &str) -> bool {
        false
    }

    fn len(&self) -> usize {
        0
    }

    fn drain(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Minimal cancel token the runtime owns, so `arkd` does not have to hand one in.
#[derive(Debug, Default)]
struct TokenCancel {
    cancelled: std::sync::atomic::AtomicBool,
}

impl CancelHandle for TokenCancel {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Bounded steering queue owned by the runtime.
#[derive(Debug)]
struct QueueSteer {
    pending: Mutex<std::collections::VecDeque<String>>,
}

impl Default for QueueSteer {
    fn default() -> Self {
        Self {
            pending: Mutex::new(std::collections::VecDeque::new()),
        }
    }
}

/// Matches `ark-agent`'s `SteerQueue::DEFAULT_CAPACITY`.
const STEER_CAPACITY: usize = 16;

impl SteerHandle for QueueSteer {
    fn push(&self, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        if pending.len() >= STEER_CAPACITY {
            return false;
        }
        pending.push_back(text.to_string());
        true
    }

    fn len(&self) -> usize {
        self.pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or(0)
    }

    fn drain(&self) -> Vec<String> {
        let Ok(mut pending) = self.pending.lock() else {
            return Vec::new();
        };
        pending.drain(..).collect()
    }
}

/// Exposed so `arkd` can reuse the same saturated-queue error text.
pub fn queue_full_error(policy: SubagentPolicy) -> ArkError {
    acquire_error(policy.max_concurrency as usize, policy.max_queued as usize)
}

/// Stop reason strings, re-exported for the boundary's response shapes.
pub fn stop_reason_str(reason: StopReason) -> &'static str {
    reason.as_str()
}

// ---------------------------------------------------------------------------
// Port impls
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl SubagentRunner for DelegationRuntime {
    fn is_enabled(&self) -> bool {
        self.policy().enabled
    }

    fn active_count(&self) -> u32 {
        self.core.active.load(Ordering::Relaxed)
    }

    async fn delegate(&self, request: SubagentRequest) -> Result<SubagentOutcome> {
        DelegationRuntime::delegate(self, request).await
    }

    async fn delegate_background(&self, request: SubagentRequest) -> Result<ChildId> {
        DelegationRuntime::delegate_background(self, request).await
    }

    async fn prepare_child(&self, request: SubagentRequest) -> Result<ChildId> {
        DelegationRuntime::prepare_child(self, request).await
    }

    async fn run_child(&self, child: ChildId) -> Result<SubagentOutcome> {
        DelegationRuntime::run_child(self, child).await
    }
}

#[async_trait::async_trait]
impl SubagentRegistry for DelegationRuntime {
    fn list(&self, parent: Option<ThreadId>) -> Vec<ChildRunRecord> {
        let mut out: Vec<ChildRunRecord> = self
            .core
            .lock_known()
            .values()
            .filter(|record| match parent {
                Some(parent) => record.parent_thread_id == parent,
                None => true,
            })
            .cloned()
            .collect();
        // Newest first, matching the list view in the Code screen.
        out.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        out
    }

    fn get(&self, child: ChildId) -> Option<ChildRunRecord> {
        self.core.lock_known().get(&child).cloned()
    }

    fn abort(&self, child: ChildId) -> bool {
        let Some(handle) = self.core.lock_handles().get(&child).cloned() else {
            return false;
        };
        handle.abort();
        true
    }

    fn steer(&self, child: ChildId, text: &str) -> bool {
        let Some(handle) = self.core.lock_handles().get(&child).cloned() else {
            return false;
        };
        handle.steer(text)
    }

    async fn destroy_child(&self, parent: ThreadId, child: ChildId) -> Result<bool> {
        DelegationRuntime::destroy_child(self, parent, child).await
    }

    fn active_count(&self) -> u32 {
        self.core.active.load(Ordering::Relaxed)
    }

    fn is_enabled(&self) -> bool {
        self.policy().enabled
    }
}

#[cfg(test)]
mod tests;
