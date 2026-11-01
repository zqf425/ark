//! `RuntimeEvent::with_seq`: the writer-assigned sequence rewrite.
//!
//! One exhaustive arm per variant, so adding a variant still forces this edit.

use super::*;

impl RuntimeEvent {
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
}
