//! Sub-agent port (design §7.8 / §7.14).
//!
//! There is still exactly **one** decision loop: `ark-agent`'s reactor. A child
//! run is that same reactor executed against a child session, not a second agent
//! implementation. This trait is the seam that lets the `delegate_task` tool ask
//! for a child run without `ark-tool` depending on the delegation runtime, the
//! store, or the model wiring.
//!
//! The runtime owns policy (enabled flag, concurrency budget, records, abort and
//! steering handles); an implementation only has to answer two questions and
//! start one run.

use ark_common::{ChildId, Result, ThreadId, TurnId, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Lifecycle of one child run. Mirrors the strings stored in
/// `children/runs.jsonl`.
pub mod status {
    pub const QUEUED: &str = "queued";
    pub const RUNNING: &str = "running";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
    pub const ABORTED: &str = "aborted";
}

/// Why a non-completed run stopped.
pub mod stop_reason {
    pub const USER: &str = "user";
    pub const ERROR: &str = "error";
    pub const BUDGET: &str = "budget";
    pub const RESTART: &str = "restart";
}

/// One delegation request.
///
/// `parent_thread_id` / `parent_turn_id` come from the caller's `ToolContext`,
/// so a child can never be attached to a thread it was not launched from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubagentRequest {
    pub parent_thread_id: ThreadId,
    pub parent_turn_id: TurnId,
    /// Short human label shown in the parent's UI.
    pub label: String,
    /// The task text handed to the child as its first user message.
    pub prompt: String,
    /// Workspace root for the child's tools; defaults to the parent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Model override; falls back to the parent turn's model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider override; falls back to the parent turn's provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Permission override (`strict`, `workspace_write`, or `unattended`);
    /// absent values inherit the parent thread's effective setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
    /// Continue an interrupted run on its original session instead of creating a
    /// new child. When set, `prompt` is the correction instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_child_id: Option<ChildId>,
}

/// What a finished child run reports back to the parent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubagentOutcome {
    pub child_id: ChildId,
    /// One of [`status`].
    pub status: String,
    /// The child's final answer. This is what the parent model reads: not the
    /// child's whole event log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub usage: Usage,
    /// Tool calls the child executed, so the parent can tell "thought" from "did".
    pub tool_invocations: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
}

impl SubagentOutcome {
    /// True when the child produced a terminal answer the parent can use.
    pub fn is_usable(&self) -> bool {
        self.status == status::COMPLETED || self.status == status::ABORTED
    }
}

/// Start and manage child runs.
#[async_trait]
pub trait SubagentRunner: Send + Sync {
    /// Whether delegation is currently switched on (a `Hot` config field).
    fn is_enabled(&self) -> bool;

    /// Children holding a concurrency slot right now.
    fn active_count(&self) -> u32;

    /// Run one child to a terminal state, enforcing the concurrency budget.
    ///
    /// Fails fast (rather than queueing forever) when the budget and the queue
    /// are both saturated, so a parent turn cannot hang on delegation.
    async fn delegate(&self, request: SubagentRequest) -> Result<SubagentOutcome>;

    /// Start one child under a runtime-owned supervisor and return immediately.
    ///
    /// The queued record is durable and the supervisor has been spawned before
    /// this returns, so the run remains abortable and converges to a terminal
    /// state even if the parent turn ends.
    async fn delegate_background(&self, request: SubagentRequest) -> Result<ChildId>;

    /// Prepare a run without starting it, returning its child id.
    ///
    /// Validation that *can* be decided immediately (disabled policy, empty
    /// prompt, unknown child, unknown parent, already completed, still active,
    /// depth violation) happens here and surfaces synchronously. The `queued`
    /// record is already durable when this returns, so an HTTP client can show
    /// the run before it has produced anything.
    ///
    /// The caller owns the execution step: it calls [`Self::run_child`] on its
    /// own task. That is what lets `POST /v1/child-runs/{id}/resume` answer
    /// immediately instead of holding the connection for a run that may take
    /// minutes.
    async fn prepare_child(&self, request: SubagentRequest) -> Result<ChildId>;

    /// Run a child prepared by [`Self::prepare_child`] to a terminal state.
    ///
    /// `NotFound` when the id was never prepared (or was already taken by
    /// another caller), so two tasks can never run the same child twice.
    async fn run_child(&self, child: ChildId) -> Result<SubagentOutcome>;
}

/// Live policy the runtime reads on every spawn (design §7.15: `subagents` is a
/// `Hot` config field, so a settings change must apply to the next child without
/// a restart).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubagentPolicy {
    pub enabled: bool,
    /// Children allowed to run at once.
    pub max_concurrency: u32,
    /// Children allowed to *wait* for a slot. Beyond this a spawn fails fast, so
    /// a parent turn can never hang on delegation.
    pub max_queued: u32,
}

impl Default for SubagentPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_concurrency: 2,
            max_queued: 20,
        }
    }
}

/// Read access to the live policy, implemented by the composition root.
pub trait SubagentPolicySource: Send + Sync {
    fn policy(&self) -> SubagentPolicy;
}

/// Management surface for child runs (used by the HTTP layer).
///
/// Kept separate from [`SubagentRunner`] on purpose: spawning a child is the
/// agent's concern, while listing/aborting/steering belongs to the boundary.
/// Both are backed by the same runtime object.
#[async_trait]
pub trait SubagentRegistry: Send + Sync {
    /// Records for one parent, or every parent when `parent` is `None`, newest
    /// first. Status reflects the latest record for each child.
    fn list(&self, parent: Option<ThreadId>) -> Vec<crate::store::ChildRunRecord>;

    /// One record by child id (latest status).
    fn get(&self, child: ChildId) -> Option<crate::store::ChildRunRecord>;

    /// Trip the child's cancel token. `false` when it is not running.
    fn abort(&self, child: ChildId) -> bool;

    /// Queue a correction for a running child. `false` when it is not running or
    /// the queue is full.
    fn steer(&self, child: ChildId, text: &str) -> bool;

    /// Destroy a child and its session, coordinated with the runtime lifecycle.
    ///
    /// `Ok(false)` means the record does not exist under `parent`. `Conflict`
    /// means it is still active and must be aborted first; the caller must not
    /// bypass this check and delete the authoritative log directly.
    async fn destroy_child(&self, parent: ThreadId, child: ChildId) -> Result<bool>;

    /// Children holding a concurrency slot right now.
    fn active_count(&self) -> u32;

    /// Whether delegation is switched on.
    fn is_enabled(&self) -> bool;
}

/// Cancel switch for one running child.
///
/// A trait rather than a concrete token so `ark-delegation` does not have to
/// depend on `ark-agent` just to abort a run: `arkd` adapts the reactor's token
/// to this shape.
pub trait CancelHandle: Send + Sync {
    fn cancel(&self);
    fn is_cancelled(&self) -> bool;
}

/// Queue for mid-run corrections (design §7.1).
///
/// `push` returns `false` when the queue is full, so the caller can report it
/// instead of silently dropping the instruction.
pub trait SteerHandle: Send + Sync {
    fn push(&self, text: &str) -> bool;
    fn len(&self) -> usize;
    /// Nothing is queued, so a drain would return an empty batch.
    ///
    /// Provided here rather than left to each implementation so that callers can
    /// ask the question through `dyn SteerHandle` without assuming `len() == 0`.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Take everything queued, in arrival order. Called by the executor at a
    /// round boundary so corrections land between model rounds, never inside a
    /// tool-call/result pair.
    fn drain(&self) -> Vec<String>;
}

impl<T: CancelHandle + ?Sized> CancelHandle for std::sync::Arc<T> {
    fn cancel(&self) {
        (**self).cancel()
    }

    fn is_cancelled(&self) -> bool {
        (**self).is_cancelled()
    }
}

impl<T: SteerHandle + ?Sized> SteerHandle for std::sync::Arc<T> {
    fn push(&self, text: &str) -> bool {
        (**self).push(text)
    }

    fn len(&self) -> usize {
        (**self).len()
    }

    fn is_empty(&self) -> bool {
        (**self).is_empty()
    }

    fn drain(&self) -> Vec<String> {
        (**self).drain()
    }
}
