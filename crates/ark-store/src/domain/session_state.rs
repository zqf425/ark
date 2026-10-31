//! `session.json`: the projection of the last turn's terminal state.
//!
//! Design 7.4.1 lists it as a *projection*, not a source of truth: it can be
//! rebuilt from `events.jsonl`, so a torn write only costs one replay.

use ark_common::TurnId;
use ark_contract::RuntimeEvent;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Terminal lifecycle state of the thread's most recent turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Running,
    Completed,
    Failed,
    Aborted,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Idle => "idle",
            SessionStatus::Running => "running",
            SessionStatus::Completed => "completed",
            SessionStatus::Failed => "failed",
            SessionStatus::Aborted => "aborted",
        }
    }

    /// Whether a turn is currently in flight for this thread.
    pub fn is_active(self) -> bool {
        matches!(self, SessionStatus::Running)
    }
}

/// Parsed `session.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_id: Option<TurnId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            status: SessionStatus::Idle,
            last_turn_id: None,
            started_at: None,
            ended_at: None,
            error: None,
            updated_at: Utc::now(),
        }
    }
}

impl SessionState {
    /// Fold one event; returns `true` when the projection changed and the file
    /// therefore needs rewriting.
    pub fn apply(&mut self, event: &RuntimeEvent) -> bool {
        let changed = match event {
            RuntimeEvent::TurnStarted {
                turn_id, timestamp, ..
            } => {
                self.status = SessionStatus::Running;
                self.last_turn_id = Some(*turn_id);
                self.started_at = Some(*timestamp);
                self.ended_at = None;
                self.error = None;
                true
            }
            RuntimeEvent::TurnCompleted {
                turn_id, timestamp, ..
            } => {
                self.status = SessionStatus::Completed;
                self.last_turn_id = Some(*turn_id);
                self.ended_at = Some(*timestamp);
                self.error = None;
                true
            }
            RuntimeEvent::TurnFailed {
                turn_id,
                timestamp,
                message,
                ..
            } => {
                self.status = SessionStatus::Failed;
                self.last_turn_id = Some(*turn_id);
                self.ended_at = Some(*timestamp);
                self.error = Some(message.clone());
                true
            }
            RuntimeEvent::TurnAborted {
                turn_id,
                timestamp,
                reason,
                ..
            } => {
                self.status = SessionStatus::Aborted;
                self.last_turn_id = Some(*turn_id);
                self.ended_at = Some(*timestamp);
                self.error = Some(reason.clone());
                true
            }
            _ => false,
        };
        if changed {
            self.updated_at = Utc::now();
        }
        changed
    }

    /// Rebuild the projection from a full event stream (used by repair).
    pub fn replay(events: &[RuntimeEvent]) -> Self {
        let mut state = Self::default();
        for event in events {
            state.apply(event);
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::{ArkErrorCode, ThreadId};

    fn thread() -> ThreadId {
        ThreadId::new()
    }

    #[test]
    fn turn_lifecycle_moves_through_states() {
        let turn = TurnId::new();
        let now = Utc::now();
        let mut state = SessionState::default();
        assert!(state.apply(&RuntimeEvent::TurnStarted {
            seq: 1,
            thread_id: thread(),
            turn_id: turn,
            timestamp: now,
            auto_continuation: false,
        }));
        assert_eq!(state.status, SessionStatus::Running);
        assert!(state.status.is_active());
        state.apply(&RuntimeEvent::TurnCompleted {
            seq: 2,
            thread_id: thread(),
            turn_id: turn,
            timestamp: now,
        });
        assert_eq!(state.status, SessionStatus::Completed);
        assert!(!state.status.is_active());
        assert_eq!(state.last_turn_id, Some(turn));
    }

    #[test]
    fn failures_keep_the_message() {
        let mut state = SessionState::default();
        state.apply(&RuntimeEvent::TurnFailed {
            seq: 3,
            thread_id: thread(),
            turn_id: TurnId::new(),
            timestamp: Utc::now(),
            message: "boom".into(),
            model_request_failed: false,
        });
        assert_eq!(state.status, SessionStatus::Failed);
        assert_eq!(state.error.as_deref(), Some("boom"));
    }

    #[test]
    fn unrelated_events_do_not_dirty_the_projection() {
        let mut state = SessionState::default();
        let before = state.status;
        let changed = state.apply(&RuntimeEvent::Error {
            seq: 4,
            thread_id: thread(),
            timestamp: Utc::now(),
            code: ArkErrorCode::InternalError,
            message: "x".into(),
        });
        assert!(!changed);
        assert_eq!(state.status, before);
    }

    #[test]
    fn replay_equals_incremental() {
        let turn = TurnId::new();
        let now = Utc::now();
        let events = vec![
            RuntimeEvent::TurnStarted {
                seq: 1,
                thread_id: thread(),
                turn_id: turn,
                timestamp: now,
                auto_continuation: false,
            },
            RuntimeEvent::TurnAborted {
                seq: 2,
                thread_id: thread(),
                turn_id: turn,
                timestamp: now,
                reason: "user".into(),
            },
        ];
        let mut incremental = SessionState::default();
        for event in &events {
            incremental.apply(event);
        }
        assert_eq!(SessionState::replay(&events).status, incremental.status);
        assert_eq!(SessionState::replay(&events).error, incremental.error);
    }
}
