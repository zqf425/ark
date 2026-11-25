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

/// `skip_serializing_if` helper for additive unsigned counters: default `0`
/// keeps old logs byte-compatible and stays parseable by ts-rs.
fn is_zero(value: &u32) -> bool {
    *value == 0
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

/// Kinds declared on the wire but with **no producer and no consumer yet**:
/// they name an event family a designed milestone still intends to emit.
///
/// The gate below is two-way: a variant that leaves this list must have grown a
/// `RuntimeEvent` arm (the serde tag check in the test fails otherwise), and a
/// new ghost variant cannot be added without being listed here explicitly.
/// Keeping the wire union complete is deliberate — the renderer's `default`
/// branch must be able to represent "server may send this one day".
pub const PLANNED_KINDS: &[EventKind] = &[
    EventKind::ToolCallReady,
    EventKind::ToolDispatchTrace,
    EventKind::ToolResultUploadWait,
    EventKind::ToolCatalogChanged,
    EventKind::PipelineStage,
    EventKind::ContextPressure,
    EventKind::BashSessionStarted,
    EventKind::BashSessionUpdated,
    EventKind::BashSessionCompleted,
    EventKind::LlmSpanStart,
    EventKind::LlmSpanEnd,
    EventKind::FingerprintDrift,
    EventKind::RecoveryTriggered,
];

impl EventKind {
    /// Every kind, in declaration order. Exhaustive on purpose: adding a
    /// variant fails to compile here, forcing the allowlist question.
    pub fn all() -> Vec<EventKind> {
        vec![
            EventKind::ThreadCreated,
            EventKind::ThreadUpdated,
            EventKind::ThreadRewound,
            EventKind::TurnStarted,
            EventKind::TurnCompleted,
            EventKind::TurnFailed,
            EventKind::TurnAborted,
            EventKind::TurnSteered,
            EventKind::ItemCreated,
            EventKind::ItemUpdated,
            EventKind::ItemCompleted,
            EventKind::AssistantTextDelta,
            EventKind::AssistantReasoningDelta,
            EventKind::ToolCallReady,
            EventKind::ToolDispatchTrace,
            EventKind::ToolResultUploadWait,
            EventKind::ToolStormSuppressed,
            EventKind::ToolCatalogChanged,
            EventKind::ToolCallStarted,
            EventKind::ToolCallFinished,
            EventKind::ApprovalRequested,
            EventKind::ApprovalResolved,
            EventKind::UserInputRequested,
            EventKind::UserInputResolved,
            EventKind::CompactionStarted,
            EventKind::CompactionCompleted,
            EventKind::GoalUpdated,
            EventKind::GoalCleared,
            EventKind::TodosUpdated,
            EventKind::TodosCleared,
            EventKind::PipelineStage,
            EventKind::ContextPressure,
            EventKind::BashSessionStarted,
            EventKind::BashSessionUpdated,
            EventKind::BashSessionCompleted,
            EventKind::Usage,
            EventKind::Error,
            EventKind::Heartbeat,
            EventKind::LlmSpanStart,
            EventKind::LlmSpanEnd,
            EventKind::FingerprintDrift,
            EventKind::RecoveryTriggered,
            EventKind::SseIdleTimeout,
            EventKind::TransientRetry,
            EventKind::ChildActivity,
            EventKind::ChildCompleted,
            EventKind::ChildCompletionAcknowledged,
            EventKind::AttachmentAssociated,
        ]
    }

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
    pub status: ark_common::ChildRunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How one referenced attachment reached (or did not reach) the model seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TurnAttachmentOutcome {
    pub attachment_id: ark_common::AttachmentId,
    /// Original upload filename, when metadata was readable. Chip labels in
    /// the renderer come from here; an id alone would be unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_filename: Option<String>,
    pub delivered: TurnAttachmentDelivery,
    /// Human-readable reason for anything that did not ride the seed as an
    /// image, e.g. the per-turn image budget or a missing model capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Delivery channel of one attachment in a turn's model seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnAttachmentDelivery {
    /// Image bytes rode the seed as multimodal parts.
    Image,
    /// Non-image attachment whose `preview_text` rode the seed as text.
    PreviewText,
    /// Image attachment downgraded to `preview_text` because the model did not
    /// declare vision support.
    Degraded,
    /// Skipped entirely, e.g. over the per-turn image budget.
    Skipped,
    /// Metadata or bytes could not be read at seed time.
    Unavailable,
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
        /// `true` when the turn is the harness retrying a previous turn that
        /// died on a transient model-request failure (gateway outage, broken
        /// stream). Carries the retry attempt number so the log records how
        /// many recoveries one user request already consumed. Additive with a
        /// serde default so pre-existing logs stay readable.
        #[serde(default, skip_serializing_if = "is_zero")]
        auto_recovery: u32,
        /// Attachments this turn referenced, already authorized against the
        /// thread by `attachment_associated` events.
        ///
        /// Phase one records usage only; since phase two the per-attachment
        /// delivery outcome rides `attachment_outcomes`. Additive with a serde
        /// default so logs written before this field existed stay readable.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachment_ids: Vec<ark_common::AttachmentId>,
        /// How each referenced attachment reached (or did not reach) the model
        /// seed. Recorded here rather than as separate events so one turn's
        /// attachment audit is a single replayable record. Additive with a
        /// serde default; empty means every attachment was delivered as an
        /// image or the turn had none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachment_outcomes: Vec<TurnAttachmentOutcome>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<ark_common::ItemId>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<ark_common::ItemId>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<ark_common::ItemId>,
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
        /// The refused call's arguments. `ToolCallStarted` already carries
        /// them for executed calls; a suppressed call must leave the same
        /// trace, or a post-hoc audit cannot tell which command repeated.
        /// Additive with a serde default so old logs stay readable.
        #[serde(default)]
        arguments: serde_json::Value,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<ark_common::ItemId>,
        timestamp: DateTime<Utc>,
        request_id: String,
        prompt: String,
    },
    UserInputResolved {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<ark_common::ItemId>,
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
        status: ark_common::ChildRunStatus,
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
        status: ark_common::ChildRunStatus,
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

mod accessors;
mod with_seq;

#[cfg(test)]
mod tests {
    #[test]
    fn as_str_and_serde_agree_for_every_kind() {
        for kind in EventKind::all() {
            let serialized = serde_json::to_value(kind).expect("serializable");
            assert_eq!(
                serialized.as_str(),
                Some(kind.as_str()),
                "as_str and serde disagree for {:?}",
                kind
            );
        }
        // The union's serialized names are unique, so a wire tag identifies
        // exactly one kind.
        let mut names: Vec<String> = EventKind::all()
            .iter()
            .map(|kind| kind.as_str().to_string())
            .collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), EventKind::all().len());
    }

    #[test]
    fn every_kind_is_produced_or_explicitly_planned() {
        for kind in EventKind::all() {
            // A bare `{"kind": ...}` payload is enough to tell a produced kind
            // from a ghost: a produced kind maps to a real `RuntimeEvent`
            // variant, so serde complains about *missing fields*; a ghost has
            // no variant at all, so serde reports *unknown variant*.
            let payload = format!("{{\"kind\":\"{}\"}}", kind.as_str());
            let error = serde_json::from_str::<RuntimeEvent>(&payload)
                .expect_err("an empty payload is never a valid event");
            let unknown_variant = error.to_string().contains("unknown variant");
            let planned = PLANNED_KINDS.contains(&kind);
            assert_eq!(
                unknown_variant,
                planned,
                "kind {}: producer/allowlist state changed; update PLANNED_KINDS",
                kind.as_str()
            );
        }
    }

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
            auto_recovery: 0,
            seq: 0,
            thread_id: ThreadId::new(),
            turn_id: ark_common::TurnId::new(),
            timestamp: Utc::now(),
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
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
            status: ark_common::ChildRunStatus::Completed,
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
