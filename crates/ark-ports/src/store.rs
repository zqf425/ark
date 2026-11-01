//! Session store port (design §7.4).
//!
//! The directory is the system of record and the index is derived. This trait
//! therefore exposes append + replay + atomic subscribe, and deliberately does
//! not expose "update the index" — that is the projector's private concern.

use ark_common::{ChildId, Result, ThreadId, TurnId};
use ark_contract::{RewindResponse, RuntimeEvent, ThreadItem, ThreadRuntimeSettings};
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

/// Latest record per child, in order of first appearance.
///
/// `children/runs.jsonl` is append-only, so one child legitimately owns several
/// rows: `queued`, `running`, and the terminal row, plus one row per `resume`
/// (which bumps `resume_count` instead of creating a second child). Every
/// projection must therefore fold before it counts or lists children — a raw
/// `SessionStore::list_child_runs` result has one row per **status change**, not
/// per child, and mapping it straight to a list multiplies the same child id.
///
/// The fold keeps the last row seen for an id (the log is ordered, so that is
/// the newest state) and preserves first-appearance order so a UI list does not
/// jump between refreshes.
pub fn latest_child_runs(records: impl IntoIterator<Item = ChildRunRecord>) -> Vec<ChildRunRecord> {
    let mut order: Vec<ChildId> = Vec::new();
    let mut latest: std::collections::HashMap<ChildId, ChildRunRecord> =
        std::collections::HashMap::new();
    for record in records {
        if !latest.contains_key(&record.child_id) {
            order.push(record.child_id);
        }
        latest.insert(record.child_id, record);
    }
    order
        .into_iter()
        .filter_map(|id| latest.remove(&id))
        .collect()
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

    /// Replay only attachment association records for a thread.
    ///
    /// Association lives exclusively in the durable event log, so the three
    /// readers (thread detail, turn-start authorization, delete guard) all go
    /// through this narrow projection instead of scanning a transcript.
    async fn replay_attachment_events(&self, thread: ThreadId) -> Result<Vec<RuntimeEvent>>;

    /// Highest durable `seq` for the thread (0 when the log is empty).
    async fn highest_seq(&self, thread: ThreadId) -> Result<u64>;

    /// Every durable event, used to rebuild render items on load.
    async fn read_items(&self, thread: ThreadId) -> Result<Vec<ThreadItem>>;

    /// Atomically watermark, read the backlog and register a live channel.
    async fn subscribe(&self, thread: ThreadId, since_seq: u64) -> Result<SubscribeTicket>;

    /// Fork a thread: copy events up to and including `seq` into a new thread.
    async fn fork_thread(&self, thread: ThreadId, upto_seq: u64) -> Result<ThreadMeta>;

    /// Physically truncate the authoritative event history at `rewind_from_seq`.
    /// The returned durable reset event lets SSE consumers discard stale state.
    async fn rewind_thread(
        &self,
        thread: ThreadId,
        rewind_from_seq: u64,
        target_turn_id: Option<ark_common::TurnId>,
    ) -> Result<RewindResponse>;

    /// Return a durable rewind marker even after ordinary event compaction.
    async fn rewind_record(
        &self,
        thread: ThreadId,
        rewind_from_seq: Option<u64>,
        target_turn_id: Option<TurnId>,
    ) -> Result<Option<RewindResponse>>;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(parent: ThreadId, child: ChildId, status: &str) -> ChildRunRecord {
        let mut record = ChildRunRecord::queued(child, parent, TurnId::new(), "scan", Utc::now());
        record.status = status.to_string();
        record
    }

    #[test]
    fn folding_a_status_transition_sequence_yields_one_row_per_child() {
        // The regression this exists for: a single delegated child that went
        // queued -> running -> completed produced three `child_ids` entries.
        let parent = ThreadId::new();
        let child = ChildId::new();
        let folded = latest_child_runs(vec![
            record(parent, child, "queued"),
            record(parent, child, "running"),
            record(parent, child, "completed"),
        ]);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].child_id, child);
        assert_eq!(folded[0].status, "completed", "the newest row wins");
    }

    #[test]
    fn a_resume_keeps_one_child_and_the_last_row_wins() {
        // `resume` appends a new row for the same child instead of creating a
        // second one, so the fold must not report the retry as a duplicate.
        let parent = ThreadId::new();
        let child = ChildId::new();
        let mut resumed = record(parent, child, "completed");
        resumed.resume_count = Some(1);
        // The rows that follow a resume are cloned from the resumed record, so
        // the count survives to the row that actually wins the fold.
        let mut running = record(parent, child, "running");
        running.resume_count = Some(1);
        let folded = latest_child_runs(vec![record(parent, child, "failed"), resumed, running]);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].status, "running");
        assert_eq!(folded[0].resume_count, Some(1));
    }

    #[test]
    fn distinct_children_keep_their_first_appearance_order() {
        let parent = ThreadId::new();
        let first = ChildId::new();
        let second = ChildId::new();
        let folded = latest_child_runs(vec![
            record(parent, first, "queued"),
            record(parent, second, "queued"),
            record(parent, first, "completed"),
        ]);
        assert_eq!(
            folded.iter().map(|r| r.child_id).collect::<Vec<_>>(),
            vec![first, second],
            "the list must not reorder between refreshes"
        );
    }
}
