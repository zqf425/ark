//! Runtime events (design appendix B).
//!
//! Every event carries the three fields the SSE boundary and the session log
//! agree on: `seq` (assigned by the single session writer), `thread_id` and
//! `timestamp`. Payload fields keep their historical names so a differential
//! test can compare old and new streams field by field.

use ark_common::{ThreadId, Usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::ThreadCreated => "thread_created",
            EventKind::ThreadUpdated => "thread_updated",
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
        }
    }
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

    // ---- turn lifecycle ----
    TurnStarted {
        #[ts(as = "f64")]
        seq: u64,
        thread_id: ThreadId,
        turn_id: ark_common::TurnId,
        timestamp: DateTime<Utc>,
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
    },
}

impl RuntimeEvent {
    pub fn seq(&self) -> u64 {
        match self {
            RuntimeEvent::ThreadCreated { seq, .. }
            | RuntimeEvent::ThreadUpdated { seq, .. }
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
            | RuntimeEvent::Usage { seq, .. }
            | RuntimeEvent::Error { seq, .. }
            | RuntimeEvent::Heartbeat { seq, .. }
            | RuntimeEvent::SseIdleTimeout { seq, .. }
            | RuntimeEvent::TransientRetry { seq, .. }
            | RuntimeEvent::CompactionStarted { seq, .. }
            | RuntimeEvent::CompactionCompleted { seq, .. }
            | RuntimeEvent::ChildActivity { seq, .. } => *seq,
        }
    }

    pub fn thread_id(&self) -> ThreadId {
        match self {
            RuntimeEvent::ThreadCreated { thread_id, .. }
            | RuntimeEvent::ThreadUpdated { thread_id, .. }
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
            | RuntimeEvent::Usage { thread_id, .. }
            | RuntimeEvent::Error { thread_id, .. }
            | RuntimeEvent::Heartbeat { thread_id, .. }
            | RuntimeEvent::SseIdleTimeout { thread_id, .. }
            | RuntimeEvent::TransientRetry { thread_id, .. }
            | RuntimeEvent::CompactionStarted { thread_id, .. }
            | RuntimeEvent::CompactionCompleted { thread_id, .. }
            | RuntimeEvent::ChildActivity { thread_id, .. } => *thread_id,
        }
    }

    pub fn kind(&self) -> EventKind {
        match self {
            RuntimeEvent::ThreadCreated { .. } => EventKind::ThreadCreated,
            RuntimeEvent::ThreadUpdated { .. } => EventKind::ThreadUpdated,
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
            RuntimeEvent::Usage { .. } => EventKind::Usage,
            RuntimeEvent::Error { .. } => EventKind::Error,
            RuntimeEvent::Heartbeat { .. } => EventKind::Heartbeat,
            RuntimeEvent::SseIdleTimeout { .. } => EventKind::SseIdleTimeout,
            RuntimeEvent::TransientRetry { .. } => EventKind::TransientRetry,
            RuntimeEvent::CompactionStarted { .. } => EventKind::CompactionStarted,
            RuntimeEvent::CompactionCompleted { .. } => EventKind::CompactionCompleted,
            RuntimeEvent::ChildActivity { .. } => EventKind::ChildActivity,
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
            RuntimeEvent::TurnStarted {
                thread_id,
                turn_id,
                timestamp,
                ..
            } => RuntimeEvent::TurnStarted {
                seq,
                thread_id,
                turn_id,
                timestamp,
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
                ..
            } => RuntimeEvent::TurnFailed {
                seq,
                thread_id,
                turn_id,
                timestamp,
                message,
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
                ..
            } => RuntimeEvent::ToolCallStarted {
                seq,
                thread_id,
                turn_id,
                timestamp,
                call_id,
                tool,
            },
            RuntimeEvent::ToolCallFinished {
                thread_id,
                turn_id,
                timestamp,
                call_id,
                tool,
                ok,
                duration_ms,
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
                ..
            } => RuntimeEvent::CompactionCompleted {
                seq,
                thread_id,
                turn_id,
                timestamp,
                removed_items,
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
        }
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            RuntimeEvent::ThreadCreated { timestamp, .. }
            | RuntimeEvent::ThreadUpdated { timestamp, .. }
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
            | RuntimeEvent::Usage { timestamp, .. }
            | RuntimeEvent::Error { timestamp, .. }
            | RuntimeEvent::Heartbeat { timestamp, .. }
            | RuntimeEvent::SseIdleTimeout { timestamp, .. }
            | RuntimeEvent::TransientRetry { timestamp, .. }
            | RuntimeEvent::CompactionStarted { timestamp, .. }
            | RuntimeEvent::CompactionCompleted { timestamp, .. }
            | RuntimeEvent::ChildActivity { timestamp, .. } => *timestamp,
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
        };
        assert_eq!(event.with_seq(42).seq(), 42);
    }
}
