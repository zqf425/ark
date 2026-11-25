//! `RuntimeEvent::with_seq`: the writer-assigned sequence rewrite.
//!
//! Every variant repeats its payload fields exactly once, one line per
//! variant: the field list *is* the information, so a macro that re-spells it
//! would only move the duplication. Adding a variant or a payload field is
//! still a deliberate edit here — the match must stay exhaustive.

use super::*;

/// Rewrites the writer-assigned `seq` of one variant, field list spelled out.
///
/// `seq: _` binds the producer-assigned placeholder; everything after it is
/// forwarded verbatim into the fresh variant.
macro_rules! rewrite_seq {
    ($event:expr, $seq:expr, $( $variant:ident { $($field:ident),* $(,)? } ),* $(,)?) => {
        match $event {
            $(
                RuntimeEvent::$variant { seq: _, $($field),* } => RuntimeEvent::$variant {
                    seq: $seq,
                    $($field),*
                },
            )*
        }
    };
}

impl RuntimeEvent {
    /// Sequence numbers are assigned by the session writer, never by producers.
    pub fn with_seq(self, seq: u64) -> Self {
        rewrite_seq!(
            self,
            seq,
            // ---- thread lifecycle ----
            ThreadCreated {
                thread_id,
                timestamp,
                title
            },
            ThreadUpdated {
                thread_id,
                timestamp,
                title
            },
            ThreadRewound {
                thread_id,
                timestamp,
                rewind_from_seq,
                target_turn_id
            },
            // ---- turn lifecycle ----
            TurnStarted {
                thread_id,
                turn_id,
                timestamp,
                auto_continuation,
                auto_recovery,
                attachment_ids,
                attachment_outcomes,
            },
            TurnCompleted {
                thread_id,
                turn_id,
                timestamp
            },
            TurnFailed {
                thread_id,
                turn_id,
                item_id,
                timestamp,
                message,
                model_request_failed
            },
            TurnAborted {
                thread_id,
                turn_id,
                timestamp,
                reason
            },
            TurnSteered {
                thread_id,
                turn_id,
                timestamp,
                text
            },
            // ---- streaming ----
            AssistantTextDelta {
                thread_id,
                turn_id,
                item_id,
                timestamp,
                delta
            },
            AssistantReasoningDelta {
                thread_id,
                turn_id,
                item_id,
                timestamp,
                delta
            },
            // ---- items ----
            ItemCreated {
                thread_id,
                turn_id,
                timestamp,
                item
            },
            ItemUpdated {
                thread_id,
                turn_id,
                timestamp,
                item
            },
            ItemCompleted {
                thread_id,
                turn_id,
                timestamp,
                item
            },
            // ---- tools ----
            ToolCallStarted {
                thread_id,
                turn_id,
                item_id,
                timestamp,
                call_id,
                tool,
                arguments
            },
            ToolCallFinished {
                thread_id,
                turn_id,
                item_id,
                timestamp,
                call_id,
                tool,
                ok,
                duration_ms,
                output,
                detail,
            },
            ToolStormSuppressed {
                thread_id,
                turn_id,
                timestamp,
                tool,
                count,
                arguments,
            },
            // ---- gates ----
            ApprovalRequested {
                thread_id,
                turn_id,
                timestamp,
                approval_id,
                tool,
                summary
            },
            ApprovalResolved {
                thread_id,
                turn_id,
                timestamp,
                approval_id,
                approved
            },
            UserInputRequested {
                thread_id,
                turn_id,
                item_id,
                timestamp,
                request_id,
                prompt
            },
            UserInputResolved {
                thread_id,
                turn_id,
                item_id,
                timestamp,
                request_id,
                answer
            },
            // ---- plan ----
            GoalUpdated {
                thread_id,
                timestamp,
                goal
            },
            GoalCleared {
                thread_id,
                timestamp
            },
            TodosUpdated {
                thread_id,
                timestamp,
                todos
            },
            TodosCleared {
                thread_id,
                timestamp
            },
            // ---- context ----
            CompactionStarted {
                thread_id,
                turn_id,
                timestamp
            },
            CompactionCompleted {
                thread_id,
                turn_id,
                timestamp,
                removed_items,
                checkpoint_chars,
            },
            // ---- progress and telemetry ----
            Usage {
                thread_id,
                turn_id,
                timestamp,
                usage
            },
            Error {
                thread_id,
                timestamp,
                code,
                message
            },
            Heartbeat {
                thread_id,
                timestamp
            },
            SseIdleTimeout {
                thread_id,
                timestamp
            },
            TransientRetry {
                thread_id,
                timestamp,
                attempt,
                message
            },
            // ---- child runs ----
            ChildActivity {
                thread_id,
                timestamp,
                child_id,
                label,
                status
            },
            ChildCompleted {
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
            ChildCompletionAcknowledged {
                thread_id,
                turn_id,
                timestamp,
                notification_id,
            },
            // ---- attachments ----
            AttachmentAssociated {
                thread_id,
                timestamp,
                attachment_id
            },
        )
    }
}
