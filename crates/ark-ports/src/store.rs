//! Session store port (design §7.4).
//!
//! The directory is the system of record and the index is derived. This trait
//! therefore exposes append + replay + atomic subscribe, and deliberately does
//! not expose "update the index" — that is the projector's private concern.

use ark_common::{ChildId, Result, ThreadId};
use ark_contract::{RuntimeEvent, ThreadItem, ThreadRuntimeSettings};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `thread.json` content plus the derived bookkeeping the list view needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadMeta {
    pub thread_id: ThreadId,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub runtime: ThreadRuntimeSettings,
}

/// Authoritative child-run record, appended to `children/runs.jsonl`.
///
/// `index.json` is only a cache rebuilt from this log, so ownership checks read
/// here rather than trusting the cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildRunRecord {
    pub child_id: ChildId,
    pub parent_thread_id: ThreadId,
    pub parent_turn_id: ark_common::TurnId,
    pub label: String,
    /// `queued` | `running` | `completed` | `failed` | `aborted`.
    ///
    /// Kept as a string so an unknown future state degrades to "unknown" in the
    /// UI instead of failing to deserialize the whole run log.
    pub status: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Explicit permission override retained for a resumed child run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
    /// The task the child was given. Persisted so a resume can replay it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The child's final answer, shown to the parent instead of its raw log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Why a non-completed run stopped (`user` | `error` | `budget` | `restart`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Usage the child reported, summed over its rounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ark_common::Usage>,
    /// Tool calls the child executed, so a parent can tell "thought" from "did".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_invocations: Option<u64>,
    /// How many times this run was resumed on its original session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_count: Option<u32>,
}

impl ChildRunRecord {
    /// A freshly queued run. The only constructor so a new required field cannot
    /// be forgotten at one of the call sites.
    pub fn queued(
        child_id: ChildId,
        parent_thread_id: ThreadId,
        parent_turn_id: ark_common::TurnId,
        label: impl Into<String>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            child_id,
            parent_thread_id,
            parent_turn_id,
            label: label.into(),
            status: "queued".to_string(),
            created_at,
            ended_at: None,
            model: None,
            provider_id: None,
            autonomy: None,
            prompt: None,
            summary: None,
            error: None,
            usage: None,
            tool_invocations: None,
            resume_count: None,
        }
    }

    /// True while the child still holds a concurrency slot.
    pub fn is_active(&self) -> bool {
        self.ended_at.is_none() && (self.status == "queued" || self.status == "running")
    }
}

/// Result of the atomic "snapshot then subscribe" operation.
pub struct SubscribeTicket {
    pub backlog: Vec<RuntimeEvent>,
    pub watermark: u64,
    pub live: tokio::sync::mpsc::Receiver<RuntimeEvent>,
}

#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    async fn create_thread(&self, title: &str) -> Result<ThreadMeta>;

    async fn list_threads(&self) -> Result<Vec<ThreadMeta>>;

    async fn get_thread(&self, thread: ThreadId) -> Result<ThreadMeta>;

    async fn rename_thread(&self, thread: ThreadId, title: &str) -> Result<ThreadMeta>;

    async fn set_archived(&self, thread: ThreadId, archived: bool) -> Result<ThreadMeta>;

    async fn set_thread_runtime(
        &self,
        thread: ThreadId,
        runtime: ThreadRuntimeSettings,
    ) -> Result<ThreadMeta>;

    async fn delete_thread(&self, thread: ThreadId) -> Result<()>;

    /// Append one event log line and return the stored event with its `seq`.
    async fn append_event(&self, thread: ThreadId, event: RuntimeEvent) -> Result<RuntimeEvent>;

    /// Replay every durable event with `seq > since_seq`, in order.
    async fn replay_events(&self, thread: ThreadId, since_seq: u64) -> Result<Vec<RuntimeEvent>>;

    /// Replay only durable child-completion lifecycle events for a parent.
    ///
    /// Completion-inbox projection and thread detail use this narrow query so
    /// an ordinary turn does not clone an unbounded transcript just to inspect
    /// `child_completed` and its acknowledgement records.
    async fn replay_child_completion_events(&self, thread: ThreadId) -> Result<Vec<RuntimeEvent>>;

    /// Replay only the turn and interactive-gate lifecycle records for a thread.
    ///
    /// Thread detail uses this bounded vocabulary to rebuild controls after a
    /// refresh without cloning ordinary transcript events into a second view.
    async fn replay_interaction_events(&self, thread: ThreadId) -> Result<Vec<RuntimeEvent>>;

    /// Highest durable `seq` for the thread (0 when the log is empty).
    async fn highest_seq(&self, thread: ThreadId) -> Result<u64>;

    /// Every durable event, used to rebuild render items on load.
    async fn read_items(&self, thread: ThreadId) -> Result<Vec<ThreadItem>>;

    /// Atomically watermark, read the backlog and register a live channel.
    async fn subscribe(&self, thread: ThreadId, since_seq: u64) -> Result<SubscribeTicket>;

    /// Fork a thread: copy events up to and including `seq` into a new thread.
    async fn fork_thread(&self, thread: ThreadId, upto_seq: u64) -> Result<ThreadMeta>;

    /// Create `threads/<parent>/children/<child>/` with its `thread.json`.
    ///
    /// Also enforces the delegation depth policy, so a child of a child is
    /// refused here rather than by a check each caller could forget.
    async fn create_child_run(&self, parent: ThreadId, child: ChildId, label: &str) -> Result<()>;

    /// Append a child-run record under the parent's `children/runs.jsonl`.
    ///
    /// The log is append-only, so a status change is a *new* record with the
    /// same `child_id`; the fold keeps the latest.
    async fn append_child_run(&self, record: ChildRunRecord) -> Result<()>;

    /// Rebuild the `child_id -> parent_thread_id` map from the authoritative log.
    async fn list_child_runs(&self, parent: ThreadId) -> Result<Vec<ChildRunRecord>>;

    /// Destroy one child run: its session directory and every record it owns.
    ///
    /// Returns `false` when `parent` has no such child, so the caller can answer
    /// 404 instead of pretending the delete happened.
    ///
    /// This is the only way a child's history is destroyed; aborting a run keeps
    /// its transcript. Because `children/runs.jsonl` is the authority, deleting
    /// one child means rewriting that log **without** the child's lines, then
    /// rebuilding the derived index — an atomic temp-and-rename, never an
    /// in-place truncation.
    async fn delete_child_run(&self, parent: ThreadId, child: ChildId) -> Result<bool>;
}
