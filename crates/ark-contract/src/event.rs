//! Runtime events (design appendix B).
//!
//! Every event carries the three fields the SSE boundary and the session log
//! agree on: `seq` (assigned by the single session writer), `thread_id` and
//! `timestamp`. Payload fields keep their historical names so a differential
//! test can compare old and new streams field by field.

use ark_common::{ThreadId, Usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `skip_serializing_if` helper for the additive boolean event fields: default
/// `false` keeps old logs byte-compatible and stays parseable by ts-rs.
fn is_false(value: &bool) -> bool {
    !*value
}

/// The full event vocabulary from appendix B.
///
/// The list is exhaustive on purpose: a renderer or a test can assert that it
/// only ever sees a known kind, and adding a kind is a deliberate edit here
/// rather than an arbitrary string at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    ThreadCreated,
    ThreadUpdated,
    ThreadRewound,
    TurnStarted,
    TurnCompleted,
    TurnFailed,
    TurnAborted,
    TurnSteered,
    ItemCreated,
    ItemUpdated,
    ItemCompleted,
    AssistantTextDelta,
    AssistantReasoningDelta,
    ToolCallReady,
    ToolDispatchTrace,
    ToolResultUploadWait,
    ToolStormSuppressed,
    ToolCatalogChanged,
    ToolCallStarted,
    ToolCallFinished,
    ApprovalRequested,
    ApprovalResolved,
    UserInputRequested,
    UserInputResolved,
    CompactionStarted,
    CompactionCompleted,
    GoalUpdated,
    GoalCleared,
    TodosUpdated,
    TodosCleared,
    PipelineStage,
    ContextPressure,
    BashSessionStarted,
    BashSessionUpdated,
    BashSessionCompleted,
    Usage,
    Error,
    Heartbeat,
    LlmSpanStart,
    LlmSpanEnd,
    FingerprintDrift,
    RecoveryTriggered,
    SseIdleTimeout,
    TransientRetry,
    ChildActivity,
    ChildCompleted,
    ChildCompletionAcknowledged,
    AttachmentAssociated,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::ThreadCreated => "thread_created",
            EventKind::ThreadUpdated => "thread_updated",
            EventKind::ThreadRewound => "thread_rewound",
            EventKind::TurnStarted => "turn_started",
            EventKind::TurnCompleted => "turn_completed",
            EventKind::TurnFailed => "turn_failed",
            EventKind::TurnAborted => "turn_aborted",
            EventKind::TurnSteered => "turn_steered",
            EventKind::ItemCreated => "item_created",
            EventKind::ItemUpdated => "item_updated",
            EventKind::ItemCompleted => "item_completed",
            EventKind::AssistantTextDelta => "assistant_text_delta",
            EventKind::AssistantReasoningDelta => "assistant_reasoning_delta",
            EventKind::ToolCallReady => "tool_call_ready",
            EventKind::ToolDispatchTrace => "tool_dispatch_trace",
            EventKind::ToolResultUploadWait => "tool_result_upload_wait",
            EventKind::ToolStormSuppressed => "tool_storm_suppressed",
            EventKind::ToolCatalogChanged => "tool_catalog_changed",
            EventKind::ToolCallStarted => "tool_call_started",
            EventKind::ToolCallFinished => "tool_call_finished",
            EventKind::ApprovalRequested => "approval_requested",
            EventKind::ApprovalResolved => "approval_resolved",
            EventKind::UserInputRequested => "user_input_requested",
            EventKind::UserInputResolved => "user_input_resolved",
            EventKind::CompactionStarted => "compaction_started",
            EventKind::CompactionCompleted => "compaction_completed",
            EventKind::GoalUpdated => "goal_updated",
            EventKind::GoalCleared => "goal_cleared",
            EventKind::TodosUpdated => "todos_updated",
            EventKind::TodosCleared => "todos_cleared",
            EventKind::PipelineStage => "pipeline_stage",
            EventKind::ContextPressure => "context_pressure",
            EventKind::BashSessionStarted => "bash_session_started",
            EventKind::BashSessionUpdated => "bash_session_updated",
            EventKind::BashSessionCompleted => "bash_session_completed",
            EventKind::Usage => "usage",
            EventKind::Error => "error",
            EventKind::Heartbeat => "heartbeat",
            EventKind::LlmSpanStart => "llm_span_start",
            EventKind::LlmSpanEnd => "llm_span_end",
            EventKind::FingerprintDrift => "fingerprint_drift",
            EventKind::RecoveryTriggered => "recovery_triggered",
            EventKind::SseIdleTimeout => "sse_idle_timeout",
            EventKind::TransientRetry => "transient_retry",
            EventKind::ChildActivity => "child_activity",
            EventKind::ChildCompleted => "child_completed",
            EventKind::ChildCompletionAcknowledged => "child_completion_acknowledged",
            EventKind::AttachmentAssociated => "attachment_associated",
        }
    }
}

/// Render-ready projection of one durable `child_completed` event.
///
/// It intentionally contains only the child terminal summary, never the child
/// transcript, so thread detail can restore completion cards without replaying
/// the whole event stream in the renderer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ChildCompletionNotice {
    #[ts(as = "f64")]
    pub seq: u64,
    pub thread_id: ThreadId,
    pub completed_at: DateTime<Utc>,
    pub notification_id: String,
    pub child_id: ark_common::ChildId,
    pub label: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A persisted runtime event.
///
/// `#[serde(tag = "kind")]` keeps the wire shape flat, which is what the
/// existing renderer expects and what the JSONL log stores verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeEvent {
    // ---- thread lifecycle ----
    ThreadCreated {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        title: String,
    },
    ThreadUpdated {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        title: String,
    },
    /// A physical history truncation. This is retained solely to notify SSE
    /// consumers that their local projection must reload from thread detail.
    ThreadRewound {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        #[ts(as = "f64")]
        rewind_from_seq: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_turn_id: Option<ark_common::TurnId>,
    },

    // ---- turn lifecycle ----
    TurnStarted {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        /// `true` when the turn was scheduled by the auto-continuation
        /// supervisor. Additive with a serde default so pre-existing logs stay
        /// readable; delivery-attempt accounting depends on it surviving.
        #[serde(default, skip_serializing_if = "is_false")]
        auto_continuation: bool,
        /// Attachments this turn referenced, already authorized against the
        /// thread by `attachment_associated` events.
        ///
        /// First phase records usage only: the ids are validated and journaled
        /// but never turned into model content. Additive with a serde default so
        /// logs written before this field existed stay readable.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachment_ids: Vec<ark_common::AttachmentId>,
    },
    TurnCompleted {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
    },
    TurnFailed {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        message: String,
        /// `true` when the failure was caused by the model request itself
        /// (for example a gateway 5xx). Additive with a serde default; the
        /// auto-continuation circuit breaker counts these, not every failure.
        #[serde(default, skip_serializing_if = "is_false")]
        model_request_failed: bool,
    },
    TurnAborted {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        reason: String,
    },
    TurnSteered {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        text: String,
    },

    // ---- streaming ----
    AssistantTextDelta {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        item_id: ark_common::ItemId,
        timestamp: DateTime<Utc>,
        delta: String,
    },
    AssistantReasoningDelta {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        item_id: ark_common::ItemId,
        timestamp: DateTime<Utc>,
        delta: String,
    },

    // ---- items ----
    ItemCreated {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        item: crate::item::ThreadItem,
    },
    ItemUpdated {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        item: crate::item::ThreadItem,
    },
    ItemCompleted {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        item: crate::item::ThreadItem,
    },

    // ---- tools ----
    ToolCallStarted {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        call_id: String,
        tool: String,
        /// The validated tool arguments, retained for Code projections.
        #[serde(default)]
        arguments: serde_json::Value,
    },
    ToolCallFinished {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        call_id: String,
        tool: String,
        ok: bool,
        #[ts(as = "f64")]
        duration_ms: u64,
        /// Capped tool output. It is already part of the model's durable turn
        /// history, so retaining it here avoids a second file/terminal parser.
        #[serde(default)]
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    ToolStormSuppressed {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        tool: String,
        #[ts(as = "f64")]
        count: u32,
    },

    // ---- gates ----
    ApprovalRequested {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        approval_id: String,
        tool: String,
        summary: String,
    },
    ApprovalResolved {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        approval_id: String,
        approved: bool,
    },
    UserInputRequested {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        request_id: String,
        prompt: String,
    },
    UserInputResolved {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        request_id: String,
        answer: String,
    },

    // ---- thread plan ----
    GoalUpdated {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        goal: String,
    },
    GoalCleared {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
    },
    TodosUpdated {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        todos: Vec<crate::dto::TodoEntry>,
    },
    TodosCleared {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
    },

    // ---- accounting ----
    Usage {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        usage: Usage,
    },

    /// A child run changed state. Published on the **parent** stream so the
    /// parent UI can show delegation progress without subscribing to the child.
    ChildActivity {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        child_id: ark_common::ChildId,
        label: String,
        status: String,
    },

    /// A child reached a terminal state. This is a durable parent-thread
    /// notification, not a second copy of the child transcript.
    ChildCompleted {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        notification_id: String,
        child_id: ark_common::ChildId,
        label: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(as = "Option<f64>")]
        tool_invocations: Option<u64>,
    },

    /// A parent turn completed after receiving the identified child result as a
    /// system source. This makes completion-inbox consumption durable.
    ChildCompletionAcknowledged {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        notification_id: String,
    },

    /// An attachment was authorized for one thread.
    ///
    /// The event log is the authoritative association source: the metadata store
    /// keeps no second association table, so a replay after a restart restores
    /// exactly the authorizations that were granted. Duplicate association is
    /// absorbed here rather than by a separate projection: a thread that already
    /// holds the association appends nothing.
    AttachmentAssociated {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        attachment_id: ark_common::AttachmentId,
    },

    // ---- diagnostics ----
    Error {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        code: ark_common::ArkErrorCode,
        message: String,
    },
    Heartbeat {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
    },
    SseIdleTimeout {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
    },
    TransientRetry {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        timestamp: DateTime<Utc>,
        #[ts(as = "f64")]
        attempt: u32,
        message: String,
    },
    CompactionStarted {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
    },
    CompactionCompleted {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
        #[ts(as = "f64")]
        removed_items: u32,
        #[serde(default)]
        #[ts(as = "f64")]
        checkpoint_chars: u32,
    },
}

impl RuntimeEvent {
    /// Whether this record participates in the durable child-completion inbox.
    ///
    /// Returns: `true` for completion and acknowledgement records.
    pub fn is_child_completion_lifecycle(&self) -> bool {
        matches!(
            self,
            RuntimeEvent::ChildCompleted { .. } | RuntimeEvent::ChildCompletionAcknowledged { .. }
        )
    }

    /// Projects a terminal child event for parent-thread detail rendering.
    ///
    /// * Returns: a completion notice for `child_completed`, otherwise `None`.
    pub fn child_completion_notice(&self) -> Option<ChildCompletionNotice> {
        match self {
            RuntimeEvent::ChildCompleted {
                seq,
                thread_id,
                timestamp,
                notification_id,
                child_id,
                label,
                status,
                summary,
                error,
                ..
            } => Some(ChildCompletionNotice {
                seq: *seq,
                thread_id: *thread_id,
                completed_at: *timestamp,
                notification_id: notification_id.clone(),
                child_id: *child_id,
                label: label.clone(),
                status: status.clone(),
                summary: summary.clone(),
                error: error.clone(),
            }),
            _ => None,
        }
    }

    pub fn seq(&self) -> u64 {
        match self {
            RuntimeEvent::ThreadCreated { seq, .. }
            | RuntimeEvent::ThreadUpdated { seq, .. }
            | RuntimeEvent::ThreadRewound { seq, .. }
            | RuntimeEvent::TurnStarted { seq, .. }
            | RuntimeEvent::TurnCompleted { seq, .. }
            | RuntimeEvent::TurnFailed { seq, .. }
            | RuntimeEvent::TurnAborted { seq, .. }
            | RuntimeEvent::TurnSteered { seq, .. }
            | RuntimeEvent::AssistantTextDelta { seq, .. }
            | RuntimeEvent::AssistantReasoningDelta { seq, .. }
            | RuntimeEvent::ItemCreated { seq, .. }
            | RuntimeEvent::ItemUpdated { seq, .. }
            | RuntimeEvent::ItemCompleted { seq, .. }
            | RuntimeEvent::ToolCallStarted { seq, .. }
            | RuntimeEvent::ToolCallFinished { seq, .. }
            | RuntimeEvent::ToolStormSuppressed { seq, .. }
            | RuntimeEvent::ApprovalRequested { seq, .. }
            | RuntimeEvent::ApprovalResolved { seq, .. }
            | RuntimeEvent::UserInputRequested { seq, .. }
            | RuntimeEvent::UserInputResolved { seq, .. }
            | RuntimeEvent::GoalUpdated { seq, .. }
            | RuntimeEvent::GoalCleared { seq, .. }
            | RuntimeEvent::TodosUpdated { seq, .. }
            | RuntimeEvent::TodosCleared { seq, .. }
            | RuntimeEvent::Usage { seq, .. }
            | RuntimeEvent::Error { seq, .. }
            | RuntimeEvent::Heartbeat { seq, .. }
            | RuntimeEvent::SseIdleTimeout { seq, .. }
            | RuntimeEvent::TransientRetry { seq, .. }
            | RuntimeEvent::CompactionStarted { seq, .. }
            | RuntimeEvent::CompactionCompleted { seq, .. }
            | RuntimeEvent::ChildActivity { seq, .. }
            | RuntimeEvent::ChildCompleted { seq, .. }
            | RuntimeEvent::ChildCompletionAcknowledged { seq, .. }
            | RuntimeEvent::AttachmentAssociated { seq, .. } => *seq,
        }
    }

    pub fn thread_id(&self) -> ThreadId {
        match self {
            RuntimeEvent::ThreadCreated { thread_id, .. }
            | RuntimeEvent::ThreadUpdated { thread_id, .. }
            | RuntimeEvent::ThreadRewound { thread_id, .. }
            | RuntimeEvent::TurnStarted { thread_id, .. }
            | RuntimeEvent::TurnCompleted { thread_id, .. }
            | RuntimeEvent::TurnFailed { thread_id, .. }
            | RuntimeEvent::TurnAborted { thread_id, .. }
            | RuntimeEvent::TurnSteered { thread_id, .. }
            | RuntimeEvent::AssistantTextDelta { thread_id, .. }
            | RuntimeEvent::AssistantReasoningDelta { thread_id, .. }
            | RuntimeEvent::ItemCreated { thread_id, .. }
            | RuntimeEvent::ItemUpdated { thread_id, .. }
            | RuntimeEvent::ItemCompleted { thread_id, .. }
            | RuntimeEvent::ToolCallStarted { thread_id, .. }
            | RuntimeEvent::ToolCallFinished { thread_id, .. }
            | RuntimeEvent::ToolStormSuppressed { thread_id, .. }
            | RuntimeEvent::ApprovalRequested { thread_id, .. }
            | RuntimeEvent::ApprovalResolved { thread_id, .. }
            | RuntimeEvent::UserInputRequested { thread_id, .. }
            | RuntimeEvent::UserInputResolved { thread_id, .. }
            | RuntimeEvent::GoalUpdated { thread_id, .. }
            | RuntimeEvent::GoalCleared { thread_id, .. }
            | RuntimeEvent::TodosUpdated { thread_id, .. }
            | RuntimeEvent::TodosCleared { thread_id, .. }
            | RuntimeEvent::Usage { thread_id, .. }
            | RuntimeEvent::Error { thread_id, .. }
            | RuntimeEvent::Heartbeat { thread_id, .. }
            | RuntimeEvent::SseIdleTimeout { thread_id, .. }
            | RuntimeEvent::TransientRetry { thread_id, .. }
            | RuntimeEvent::CompactionStarted { thread_id, .. }
            | RuntimeEvent::CompactionCompleted { thread_id, .. }
            | RuntimeEvent::ChildActivity { thread_id, .. }
            | RuntimeEvent::ChildCompleted { thread_id, .. }
            | RuntimeEvent::ChildCompletionAcknowledged { thread_id, .. }
            | RuntimeEvent::AttachmentAssociated { thread_id, .. } => *thread_id,
        }
    }

    pub fn kind(&self) -> EventKind {
        match self {
            RuntimeEvent::ThreadCreated { .. } => EventKind::ThreadCreated,
            RuntimeEvent::ThreadUpdated { .. } => EventKind::ThreadUpdated,
            RuntimeEvent::ThreadRewound { .. } => EventKind::ThreadRewound,
            RuntimeEvent::TurnStarted { .. } => EventKind::TurnStarted,
            RuntimeEvent::TurnCompleted { .. } => EventKind::TurnCompleted,
            RuntimeEvent::TurnFailed { .. } => EventKind::TurnFailed,
            RuntimeEvent::TurnAborted { .. } => EventKind::TurnAborted,
            RuntimeEvent::TurnSteered { .. } => EventKind::TurnSteered,
            RuntimeEvent::AssistantTextDelta { .. } => EventKind::AssistantTextDelta,
            RuntimeEvent::AssistantReasoningDelta { .. } => EventKind::AssistantReasoningDelta,
            RuntimeEvent::ItemCreated { .. } => EventKind::ItemCreated,
            RuntimeEvent::ItemUpdated { .. } => EventKind::ItemUpdated,
            RuntimeEvent::ItemCompleted { .. } => EventKind::ItemCompleted,
            RuntimeEvent::ToolCallStarted { .. } => EventKind::ToolCallStarted,
            RuntimeEvent::ToolCallFinished { .. } => EventKind::ToolCallFinished,
            RuntimeEvent::ToolStormSuppressed { .. } => EventKind::ToolStormSuppressed,
            RuntimeEvent::ApprovalRequested { .. } => EventKind::ApprovalRequested,
            RuntimeEvent::ApprovalResolved { .. } => EventKind::ApprovalResolved,
            RuntimeEvent::UserInputRequested { .. } => EventKind::UserInputRequested,
            RuntimeEvent::UserInputResolved { .. } => EventKind::UserInputResolved,
            RuntimeEvent::GoalUpdated { .. } => EventKind::GoalUpdated,
            RuntimeEvent::GoalCleared { .. } => EventKind::GoalCleared,
            RuntimeEvent::TodosUpdated { .. } => EventKind::TodosUpdated,
            RuntimeEvent::TodosCleared { .. } => EventKind::TodosCleared,
            RuntimeEvent::Usage { .. } => EventKind::Usage,
            RuntimeEvent::Error { .. } => EventKind::Error,
            RuntimeEvent::Heartbeat { .. } => EventKind::Heartbeat,
            RuntimeEvent::SseIdleTimeout { .. } => EventKind::SseIdleTimeout,
            RuntimeEvent::TransientRetry { .. } => EventKind::TransientRetry,
            RuntimeEvent::CompactionStarted { .. } => EventKind::CompactionStarted,
            RuntimeEvent::CompactionCompleted { .. } => EventKind::CompactionCompleted,
            RuntimeEvent::ChildActivity { .. } => EventKind::ChildActivity,
            RuntimeEvent::ChildCompleted { .. } => EventKind::ChildCompleted,
            RuntimeEvent::ChildCompletionAcknowledged { .. } => {
                EventKind::ChildCompletionAcknowledged
            }
            RuntimeEvent::AttachmentAssociated { .. } => EventKind::AttachmentAssociated,
        }
    }

    /// Sequence numbers are assigned by the session writer, never by producers.
    pub fn with_seq(self, seq: u64) -> Self {
        match self {
            RuntimeEvent::ThreadCreated {
                thread_id,
                timestamp,
                title,
                ..
            } => RuntimeEvent::ThreadCreated {
                seq,
                thread_id,
                timestamp,
                title,
            },
            RuntimeEvent::ThreadUpdated {
                thread_id,
                timestamp,
                title,
                ..
            } => RuntimeEvent::ThreadUpdated {
                seq,
                thread_id,
                timestamp,
                title,
            },
            RuntimeEvent::ThreadRewound {
                thread_id,
                timestamp,
                rewind_from_seq,
                target_turn_id,
                ..
            } => RuntimeEvent::ThreadRewound {
                seq,
                thread_id,
                timestamp,
                rewind_from_seq,
                target_turn_id,
            },
            RuntimeEvent::TurnStarted {
                thread_id,
                turn_id,
                timestamp,
                auto_continuation,
                attachment_ids,
                ..
            } => RuntimeEvent::TurnStarted {
                seq,
                thread_id,
                turn_id,
                timestamp,
                auto_continuation,
                attachment_ids,
            },
            RuntimeEvent::TurnCompleted {
                thread_id,
                turn_id,
                timestamp,
                ..
            } => RuntimeEvent::TurnCompleted {
                seq,
                thread_id,
                turn_id,
                timestamp,
            },
            RuntimeEvent::TurnFailed {
                thread_id,
                turn_id,
                timestamp,
                message,
                model_request_failed,
                ..
            } => RuntimeEvent::TurnFailed {
                seq,
                thread_id,
                turn_id,
                timestamp,
                message,
                model_request_failed,
            },
            RuntimeEvent::TurnAborted {
                thread_id,
                turn_id,
                timestamp,
                reason,
                ..
            } => RuntimeEvent::TurnAborted {
                seq,
                thread_id,
                turn_id,
                timestamp,
                reason,
            },
            RuntimeEvent::TurnSteered {
                thread_id,
                turn_id,
                timestamp,
                text,
                ..
            } => RuntimeEvent::TurnSteered {
                seq,
                thread_id,
                turn_id,
                timestamp,
                text,
            },
            RuntimeEvent::AssistantTextDelta {
                thread_id,
                turn_id,
                item_id,
                timestamp,
                delta,
                ..
            } => RuntimeEvent::AssistantTextDelta {
                seq,
                thread_id,
                turn_id,
                item_id,
                timestamp,
                delta,
            },
            RuntimeEvent::AssistantReasoningDelta {
                thread_id,
                turn_id,
                item_id,
                timestamp,
                delta,
                ..
            } => RuntimeEvent::AssistantReasoningDelta {
                seq,
                thread_id,
                turn_id,
                item_id,
                timestamp,
                delta,
            },
            RuntimeEvent::ItemCreated {
                thread_id,
                turn_id,
                timestamp,
                item,
                ..
            } => RuntimeEvent::ItemCreated {
                seq,
                thread_id,
                turn_id,
                timestamp,
                item,
            },
            RuntimeEvent::ItemUpdated {
                thread_id,
                turn_id,
                timestamp,
                item,
                ..
            } => RuntimeEvent::ItemUpdated {
                seq,
                thread_id,
                turn_id,
                timestamp,
                item,
            },
            RuntimeEvent::ItemCompleted {
                thread_id,
                turn_id,
                timestamp,
                item,
                ..
            } => RuntimeEvent::ItemCompleted {
                seq,
                thread_id,
                turn_id,
                timestamp,
                item,
            },
            RuntimeEvent::ToolCallStarted {
                thread_id,
                turn_id,
                timestamp,
                call_id,
                tool,
                arguments,
                ..
            } => RuntimeEvent::ToolCallStarted {
                seq,
                thread_id,
                turn_id,
                timestamp,
                call_id,
                tool,
                arguments,
            },
            RuntimeEvent::ToolCallFinished {
                thread_id,
                turn_id,
                timestamp,
                call_id,
                tool,
                ok,
                duration_ms,
                output,
                detail,
                ..
            } => RuntimeEvent::ToolCallFinished {
                seq,
                thread_id,
                turn_id,
                timestamp,
                call_id,
                tool,
                ok,
                duration_ms,
                output,
                detail,
            },
            RuntimeEvent::ToolStormSuppressed {
                thread_id,
                turn_id,
                timestamp,
                tool,
                count,
                ..
            } => RuntimeEvent::ToolStormSuppressed {
                seq,
                thread_id,
                turn_id,
                timestamp,
                tool,
                count,
            },
            RuntimeEvent::ApprovalRequested {
                thread_id,
                turn_id,
                timestamp,
                approval_id,
                tool,
                summary,
                ..
            } => RuntimeEvent::ApprovalRequested {
                seq,
                thread_id,
                turn_id,
                timestamp,
                approval_id,
                tool,
                summary,
            },
            RuntimeEvent::ApprovalResolved {
                thread_id,
                turn_id,
                timestamp,
                approval_id,
                approved,
                ..
            } => RuntimeEvent::ApprovalResolved {
                seq,
                thread_id,
                turn_id,
                timestamp,
                approval_id,
                approved,
            },
            RuntimeEvent::UserInputRequested {
                thread_id,
                turn_id,
                timestamp,
                request_id,
                prompt,
                ..
            } => RuntimeEvent::UserInputRequested {
                seq,
                thread_id,
                turn_id,
                timestamp,
                request_id,
                prompt,
            },
            RuntimeEvent::UserInputResolved {
                thread_id,
                turn_id,
                timestamp,
                request_id,
                answer,
                ..
            } => RuntimeEvent::UserInputResolved {
                seq,
                thread_id,
                turn_id,
                timestamp,
                request_id,
                answer,
            },
            RuntimeEvent::GoalUpdated {
                thread_id,
                timestamp,
                goal,
                ..
            } => RuntimeEvent::GoalUpdated {
                seq,
                thread_id,
                timestamp,
                goal,
            },
            RuntimeEvent::GoalCleared {
                thread_id,
                timestamp,
                ..
            } => RuntimeEvent::GoalCleared {
                seq,
                thread_id,
                timestamp,
            },
            RuntimeEvent::TodosUpdated {
                thread_id,
                timestamp,
                todos,
                ..
            } => RuntimeEvent::TodosUpdated {
                seq,
                thread_id,
                timestamp,
                todos,
            },
            RuntimeEvent::TodosCleared {
                thread_id,
                timestamp,
                ..
            } => RuntimeEvent::TodosCleared {
                seq,
                thread_id,
                timestamp,
            },
            RuntimeEvent::Usage {
                thread_id,
                turn_id,
                timestamp,
                usage,
                ..
            } => RuntimeEvent::Usage {
                seq,
                thread_id,
                turn_id,
                timestamp,
                usage,
            },
            RuntimeEvent::Error {
                thread_id,
                timestamp,
                code,
                message,
                ..
            } => RuntimeEvent::Error {
                seq,
                thread_id,
                timestamp,
                code,
                message,
            },
            RuntimeEvent::Heartbeat {
                thread_id,
                timestamp,
                ..
            } => RuntimeEvent::Heartbeat {
                seq,
                thread_id,
                timestamp,
            },
            RuntimeEvent::SseIdleTimeout {
                thread_id,
                timestamp,
                ..
            } => RuntimeEvent::SseIdleTimeout {
                seq,
                thread_id,
                timestamp,
            },
            RuntimeEvent::TransientRetry {
                thread_id,
                timestamp,
                attempt,
                message,
                ..
            } => RuntimeEvent::TransientRetry {
                seq,
                thread_id,
                timestamp,
                attempt,
                message,
            },
            RuntimeEvent::CompactionStarted {
                thread_id,
                turn_id,
                timestamp,
                ..
            } => RuntimeEvent::CompactionStarted {
                seq,
                thread_id,
                turn_id,
                timestamp,
            },
            RuntimeEvent::CompactionCompleted {
                thread_id,
                turn_id,
                timestamp,
                removed_items,
                checkpoint_chars,
                ..
            } => RuntimeEvent::CompactionCompleted {
                seq,
                thread_id,
                turn_id,
                timestamp,
                removed_items,
                checkpoint_chars,
            },
            RuntimeEvent::ChildActivity {
                thread_id,
                timestamp,
                child_id,
                label,
                status,
                ..
            } => RuntimeEvent::ChildActivity {
                seq,
                thread_id,
                timestamp,
                child_id,
                label,
                status,
            },
            RuntimeEvent::ChildCompleted {
                thread_id,
                timestamp,
                notification_id,
                child_id,
                label,
                status,
                summary,
                error,
                usage,
                tool_invocations,
                ..
            } => RuntimeEvent::ChildCompleted {
                seq,
                thread_id,
                timestamp,
                notification_id,
                child_id,
                label,
                status,
                summary,
                error,
                usage,
                tool_invocations,
            },
            RuntimeEvent::ChildCompletionAcknowledged {
                thread_id,
                turn_id,
                timestamp,
                notification_id,
                ..
            } => RuntimeEvent::ChildCompletionAcknowledged {
                seq,
                thread_id,
                turn_id,
                timestamp,
                notification_id,
            },
            RuntimeEvent::AttachmentAssociated {
                thread_id,
                timestamp,
                attachment_id,
                ..
            } => RuntimeEvent::AttachmentAssociated {
                seq,
                thread_id,
                timestamp,
                attachment_id,
            },
        }
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            RuntimeEvent::ThreadCreated { timestamp, .. }
            | RuntimeEvent::ThreadUpdated { timestamp, .. }
            | RuntimeEvent::ThreadRewound { timestamp, .. }
            | RuntimeEvent::TurnStarted { timestamp, .. }
            | RuntimeEvent::TurnCompleted { timestamp, .. }
            | RuntimeEvent::TurnFailed { timestamp, .. }
            | RuntimeEvent::TurnAborted { timestamp, .. }
            | RuntimeEvent::TurnSteered { timestamp, .. }
            | RuntimeEvent::AssistantTextDelta { timestamp, .. }
            | RuntimeEvent::AssistantReasoningDelta { timestamp, .. }
            | RuntimeEvent::ItemCreated { timestamp, .. }
            | RuntimeEvent::ItemUpdated { timestamp, .. }
            | RuntimeEvent::ItemCompleted { timestamp, .. }
            | RuntimeEvent::ToolCallStarted { timestamp, .. }
            | RuntimeEvent::ToolCallFinished { timestamp, .. }
            | RuntimeEvent::ToolStormSuppressed { timestamp, .. }
            | RuntimeEvent::ApprovalRequested { timestamp, .. }
            | RuntimeEvent::ApprovalResolved { timestamp, .. }
            | RuntimeEvent::UserInputRequested { timestamp, .. }
            | RuntimeEvent::UserInputResolved { timestamp, .. }
            | RuntimeEvent::GoalUpdated { timestamp, .. }
            | RuntimeEvent::GoalCleared { timestamp, .. }
            | RuntimeEvent::TodosUpdated { timestamp, .. }
            | RuntimeEvent::TodosCleared { timestamp, .. }
            | RuntimeEvent::Usage { timestamp, .. }
            | RuntimeEvent::Error { timestamp, .. }
            | RuntimeEvent::Heartbeat { timestamp, .. }
            | RuntimeEvent::SseIdleTimeout { timestamp, .. }
            | RuntimeEvent::TransientRetry { timestamp, .. }
            | RuntimeEvent::CompactionStarted { timestamp, .. }
            | RuntimeEvent::CompactionCompleted { timestamp, .. }
            | RuntimeEvent::ChildActivity { timestamp, .. }
            | RuntimeEvent::ChildCompleted { timestamp, .. }
            | RuntimeEvent::ChildCompletionAcknowledged { timestamp, .. }
            | RuntimeEvent::AttachmentAssociated { timestamp, .. } => *timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_strings_match_the_appendix() {
        assert_eq!(EventKind::ToolCallFinished.as_str(), "tool_call_finished");
        assert_eq!(EventKind::SseIdleTimeout.as_str(), "sse_idle_timeout");
        assert_eq!(
            EventKind::AssistantReasoningDelta.as_str(),
            "assistant_reasoning_delta"
        );
    }

    #[test]
    fn wire_shape_is_flat_with_kind_tag() {
        let event = RuntimeEvent::Heartbeat {
            seq: 1,
            thread_id: ThreadId::new(),
            timestamp: Utc::now(),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["kind"], "heartbeat");
        assert_eq!(value["seq"], 1);

        let back: RuntimeEvent = serde_json::from_value(value).unwrap();
        assert_eq!(back.kind(), EventKind::Heartbeat);
    }

    #[test]
    fn seq_can_be_reassigned_by_the_writer() {
        let event = RuntimeEvent::TurnStarted {
            seq: 0,
            thread_id: ThreadId::new(),
            turn_id: ark_common::TurnId::new(),
            timestamp: Utc::now(),
            auto_continuation: false,
            attachment_ids: Vec::new(),
        };
        assert_eq!(event.with_seq(42).seq(), 42);
    }

    #[test]
    fn compaction_completed_without_checkpoint_chars_remains_readable() {
        let thread_id = ThreadId::new();
        let turn_id = ark_common::TurnId::new();
        let timestamp = Utc::now();
        let mut old_event = serde_json::to_value(RuntimeEvent::CompactionCompleted {
            seq: 17,
            thread_id,
            turn_id,
            timestamp,
            removed_items: 3,
            checkpoint_chars: 42,
        })
        .expect("compaction event serializes");
        old_event
            .as_object_mut()
            .expect("event is a JSON object")
            .remove("checkpoint_chars");

        let parsed: RuntimeEvent =
            serde_json::from_value(old_event).expect("old compaction event remains readable");
        assert!(matches!(
            parsed,
            RuntimeEvent::CompactionCompleted {
                seq: 17,
                thread_id: parsed_thread_id,
                turn_id: parsed_turn_id,
                timestamp: parsed_timestamp,
                removed_items: 3,
                checkpoint_chars: 0,
            } if parsed_thread_id == thread_id && parsed_turn_id == turn_id && parsed_timestamp == timestamp
        ));
    }

    #[test]
    fn child_completion_event_projects_a_renderable_notice() {
        let thread_id = ThreadId::new();
        let event = RuntimeEvent::ChildCompleted {
            seq: 7,
            thread_id,
            timestamp: Utc::now(),
            notification_id: "child:result".to_string(),
            child_id: ark_common::ChildId::new(),
            label: "inspect runtime".to_string(),
            status: "completed".to_string(),
            summary: Some("The runtime is healthy.".to_string()),
            error: None,
            usage: None,
            tool_invocations: None,
        };

        let notice = event.child_completion_notice().expect("completion notice");
        assert_eq!(notice.notification_id, "child:result");
        assert_eq!(notice.thread_id, thread_id);
        assert_eq!(notice.seq, 7);
        assert_eq!(notice.summary.as_deref(), Some("The runtime is healthy."));
    }
}
