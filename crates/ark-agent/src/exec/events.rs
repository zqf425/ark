//! Constructors for the durable events a turn emits (design §7.9 / appendix B).
//!
//! The reactor never builds a `RuntimeEvent` inline: every event the agent can
//! write is constructed here, so the field set and the `seq = 0` convention
//! ("the writer assigns it") live in exactly one place.

use ark_common::{ItemId, ThreadId, TurnId, Usage};
use ark_contract::{RuntimeEvent, ThreadItem};
use chrono::Utc;

/// Namespace of event constructors for one `(thread, turn)` pair.
#[derive(Debug, Clone, Copy)]
pub struct TurnEvents {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
}

impl TurnEvents {
    pub fn new(thread_id: ThreadId, turn_id: TurnId) -> Self {
        Self { thread_id, turn_id }
    }

    /// Sequence numbers are assigned by the session writer, never by producers.
    const UNASSIGNED: u64 = 0;

    pub fn turn_started(&self) -> RuntimeEvent {
        RuntimeEvent::TurnStarted {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
        }
    }

    pub fn turn_completed(&self) -> RuntimeEvent {
        RuntimeEvent::TurnCompleted {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
        }
    }

    pub fn turn_failed(&self, message: impl Into<String>) -> RuntimeEvent {
        RuntimeEvent::TurnFailed {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            message: message.into(),
        }
    }

    pub fn turn_aborted(&self, reason: impl Into<String>) -> RuntimeEvent {
        RuntimeEvent::TurnAborted {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            reason: reason.into(),
        }
    }

    pub fn text_delta(&self, item_id: ItemId, delta: impl Into<String>) -> RuntimeEvent {
        RuntimeEvent::AssistantTextDelta {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            item_id,
            timestamp: Utc::now(),
            delta: delta.into(),
        }
    }

    pub fn reasoning_delta(&self, item_id: ItemId, delta: impl Into<String>) -> RuntimeEvent {
        RuntimeEvent::AssistantReasoningDelta {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            item_id,
            timestamp: Utc::now(),
            delta: delta.into(),
        }
    }

    pub fn item_created(&self, item: ThreadItem) -> RuntimeEvent {
        RuntimeEvent::ItemCreated {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            item,
        }
    }

    pub fn item_updated(&self, item: ThreadItem) -> RuntimeEvent {
        RuntimeEvent::ItemUpdated {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            item,
        }
    }

    pub fn item_completed(&self, item: ThreadItem) -> RuntimeEvent {
        RuntimeEvent::ItemCompleted {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            item,
        }
    }

    pub fn tool_call_started(&self, call_id: &str, tool: &str) -> RuntimeEvent {
        RuntimeEvent::ToolCallStarted {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            call_id: call_id.to_string(),
            tool: tool.to_string(),
        }
    }

    pub fn tool_call_finished(
        &self,
        call_id: &str,
        tool: &str,
        ok: bool,
        duration_ms: u64,
    ) -> RuntimeEvent {
        RuntimeEvent::ToolCallFinished {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            ok,
            duration_ms,
        }
    }

    pub fn tool_storm_suppressed(&self, tool: &str, count: u32) -> RuntimeEvent {
        RuntimeEvent::ToolStormSuppressed {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            tool: tool.to_string(),
            count,
        }
    }

    pub fn approval_requested(&self, approval_id: &str, tool: &str, summary: &str) -> RuntimeEvent {
        RuntimeEvent::ApprovalRequested {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            approval_id: approval_id.to_string(),
            tool: tool.to_string(),
            summary: summary.to_string(),
        }
    }

    pub fn approval_resolved(&self, approval_id: &str, approved: bool) -> RuntimeEvent {
        RuntimeEvent::ApprovalResolved {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            approval_id: approval_id.to_string(),
            approved,
        }
    }

    pub fn user_input_requested(&self, request_id: &str, prompt: &str) -> RuntimeEvent {
        RuntimeEvent::UserInputRequested {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            request_id: request_id.to_string(),
            prompt: prompt.to_string(),
        }
    }

    pub fn user_input_resolved(&self, request_id: &str, answer: &str) -> RuntimeEvent {
        RuntimeEvent::UserInputResolved {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            request_id: request_id.to_string(),
            answer: answer.to_string(),
        }
    }

    pub fn compaction_started(&self) -> RuntimeEvent {
        RuntimeEvent::CompactionStarted {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
        }
    }

    pub fn compaction_completed(&self, removed_items: u32) -> RuntimeEvent {
        RuntimeEvent::CompactionCompleted {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            removed_items,
        }
    }

    pub fn usage(&self, usage: Usage) -> RuntimeEvent {
        RuntimeEvent::Usage {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            usage,
        }
    }

    /// A correction injected into the running turn.
    ///
    /// Emitted *after* the text has actually been pushed into the model
    /// history, so the journal never claims a correction the model never saw.
    pub fn turn_steered(&self, text: &str) -> RuntimeEvent {
        RuntimeEvent::TurnSteered {
            seq: Self::UNASSIGNED,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            timestamp: Utc::now(),
            text: text.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::{item::ItemKind, EventKind};

    fn events() -> TurnEvents {
        TurnEvents::new(ThreadId::new(), TurnId::new())
    }

    #[test]
    fn producers_never_assign_sequence_numbers() {
        let events = events();
        let all = [
            events.turn_started(),
            events.turn_completed(),
            events.turn_failed("x"),
            events.turn_aborted("y"),
            events.text_delta(ItemId::new(), "d"),
            events.reasoning_delta(ItemId::new(), "r"),
            events.tool_call_started("c", "read"),
            events.tool_call_finished("c", "read", true, 5),
            events.usage(Usage::new(1, 1)),
        ];
        for event in all {
            assert_eq!(
                event.seq(),
                0,
                "writer assigns seq: {}",
                event.kind().as_str()
            );
        }
    }

    #[test]
    fn every_event_carries_the_turn_thread() {
        let events = events();
        assert_eq!(events.turn_started().thread_id(), events.thread_id);
        assert_eq!(events.usage(Usage::default()).thread_id(), events.thread_id);
    }

    #[test]
    fn kinds_match_their_constructor() {
        let events = events();
        assert_eq!(events.turn_completed().kind(), EventKind::TurnCompleted);
        assert_eq!(events.turn_aborted("r").kind(), EventKind::TurnAborted);
        assert_eq!(
            events.tool_storm_suppressed("read", 3).kind(),
            EventKind::ToolStormSuppressed
        );
        assert_eq!(
            events.approval_resolved("a", true).kind(),
            EventKind::ApprovalResolved
        );
        assert_eq!(
            events.user_input_resolved("r", "a").kind(),
            EventKind::UserInputResolved
        );
        assert_eq!(
            events.compaction_completed(2).kind(),
            EventKind::CompactionCompleted
        );
    }

    #[test]
    fn item_events_carry_the_item() {
        let events = events();
        let item = ThreadItem::new(events.turn_id, ItemKind::ToolCall);
        let event = events.item_created(item.clone());
        assert_eq!(event.kind(), EventKind::ItemCreated);
        match event {
            RuntimeEvent::ItemCreated {
                item: stored,
                turn_id,
                ..
            } => {
                assert_eq!(stored.item_id, item.item_id);
                assert_eq!(turn_id, events.turn_id);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn terminal_events_are_serialisable() {
        let json = serde_json::to_value(events().turn_aborted("interrupted")).unwrap();
        assert_eq!(json["kind"], "turn_aborted");
        assert_eq!(json["reason"], "interrupted");
        assert_eq!(json["seq"], 0);
    }
}
