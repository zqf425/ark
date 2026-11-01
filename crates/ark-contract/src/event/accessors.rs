//! `RuntimeEvent` accessors: child-completion projection, `seq`, `thread_id`,
//! `kind` and `timestamp`. Pure reads; the event enum lives in the parent module.

use super::*;

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
