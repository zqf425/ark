//! Pure delegation rules: the concurrency gate and the lifecycle vocabulary.
//!
//! Nothing here performs I/O or touches a store, so every rule is unit-testable
//! without a filesystem.

pub mod gate;

pub use gate::{acquire_error, SlotGate, SlotGuard, ERR_QUEUE_FULL};

use ark_common::{ChildId, ThreadId, TurnId, Usage};
use ark_ports::{CancelHandle, ChildRunRecord, SteerHandle};
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::sync::Arc;

/// Why a non-completed child stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// A human (or the parent agent) cancelled it.
    User,
    /// The child's own turn failed.
    Error,
    /// A budget (rounds, tokens, wall clock) was exhausted.
    Budget,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::User => ark_ports::stop_reason::USER,
            StopReason::Error => ark_ports::stop_reason::ERROR,
            StopReason::Budget => ark_ports::stop_reason::BUDGET,
        }
    }
}

/// Terminal state a child reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildTurnStatus {
    Completed,
    Failed { message: String },
    Aborted { reason: String },
}

impl ChildTurnStatus {
    /// Record status string (`completed` | `failed` | `aborted`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ChildTurnStatus::Completed => ark_ports::child_status::COMPLETED,
            ChildTurnStatus::Failed { .. } => ark_ports::child_status::FAILED,
            ChildTurnStatus::Aborted { .. } => ark_ports::child_status::ABORTED,
        }
    }

    /// Why it stopped, when it did not complete.
    pub fn stop_reason(&self) -> Option<StopReason> {
        match self {
            ChildTurnStatus::Completed => None,
            ChildTurnStatus::Failed { .. } => Some(StopReason::Error),
            ChildTurnStatus::Aborted { .. } => Some(StopReason::User),
        }
    }
}

/// Everything a child turn needs. Assembled by the runtime, consumed by the
/// executor that `arkd` provides.
///
/// The executor is a port because only the composition root may wire a model
/// client, a tool runner and a session writer together; `ark-delegation` stays
/// free of those crates.
#[derive(Clone)]
pub struct ChildTurnContext {
    pub child_id: ChildId,
    pub parent_thread_id: ThreadId,
    pub parent_turn_id: TurnId,
    /// The task text; on a resume this is the correction instruction.
    pub prompt: String,
    /// Tool workspace for the child. `None` means "inherit the parent's".
    pub workspace: Option<PathBuf>,
    pub model: Option<String>,
    pub provider_id: Option<String>,
    /// Permission override; `None` inherits the parent thread's setting.
    pub autonomy: Option<String>,
    /// Trips when the child is aborted (explicit abort, or parent shutdown).
    pub cancel: Arc<dyn CancelHandle>,
    /// Mid-run corrections, drained by the reactor at a round boundary.
    pub steer: Arc<dyn SteerHandle>,
}

impl std::fmt::Debug for ChildTurnContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildTurnContext")
            .field("child_id", &self.child_id)
            .field("parent_thread_id", &self.parent_thread_id)
            .field("prompt_len", &self.prompt.len())
            .finish_non_exhaustive()
    }
}

/// What the executor reports when the child's turn ended.
#[derive(Debug, Clone)]
pub struct ChildTurnReport {
    pub status: ChildTurnStatus,
    /// The child's final answer, shown to the parent instead of its raw log.
    pub summary: String,
    pub usage: Usage,
    pub tool_invocations: u64,
}

impl ChildTurnReport {
    pub fn completed(summary: impl Into<String>) -> Self {
        Self {
            status: ChildTurnStatus::Completed,
            summary: summary.into(),
            usage: Usage::default(),
            tool_invocations: 0,
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            status: ChildTurnStatus::Failed {
                message: message.into(),
            },
            summary: String::new(),
            usage: Usage::default(),
            tool_invocations: 0,
        }
    }

    pub fn aborted(reason: impl Into<String>) -> Self {
        Self {
            status: ChildTurnStatus::Aborted {
                reason: reason.into(),
            },
            summary: String::new(),
            usage: Usage::default(),
            tool_invocations: 0,
        }
    }
}

/// Fold `record` into the terminal record for `report`.
///
/// The run log is append-only, so finishing a run is a *new* record with the same
/// `child_id`; readers keep the latest. Keeping that rule here means the state
/// machine cannot accidentally mutate history.
pub fn finished_record(
    mut record: ChildRunRecord,
    report: &ChildTurnReport,
    ended_at: DateTime<Utc>,
) -> ChildRunRecord {
    record.status = report.status.as_str().to_string();
    record.ended_at = Some(ended_at);
    record.summary = Some(report.summary.clone());
    record.usage = Some(report.usage.clone());
    record.tool_invocations = Some(report.tool_invocations);
    record.error = match &report.status {
        ChildTurnStatus::Completed => None,
        ChildTurnStatus::Failed { message } => Some(message.clone()),
        ChildTurnStatus::Aborted { reason } => Some(reason.clone()),
    };
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ChildRunRecord {
        ChildRunRecord::queued(
            ChildId::new(),
            ThreadId::new(),
            TurnId::new(),
            "scan",
            Utc::now(),
        )
    }

    #[test]
    fn a_completed_run_carries_no_error() {
        let report = ChildTurnReport::completed("done");
        let finished = finished_record(record(), &report, Utc::now());
        assert_eq!(finished.status, "completed");
        assert!(finished.error.is_none());
        assert_eq!(finished.summary.as_deref(), Some("done"));
        assert!(finished.ended_at.is_some());
    }

    #[test]
    fn a_failed_run_keeps_its_message() {
        let report = ChildTurnReport::failed("provider 500");
        let finished = finished_record(record(), &report, Utc::now());
        assert_eq!(finished.status, "failed");
        assert_eq!(finished.error.as_deref(), Some("provider 500"));
        assert_eq!(
            report.status.stop_reason().map(StopReason::as_str),
            Some("error")
        );
    }

    #[test]
    fn an_aborted_run_records_the_user_stop_reason() {
        let report = ChildTurnReport::aborted("stopped by user");
        let finished = finished_record(record(), &report, Utc::now());
        assert_eq!(finished.status, "aborted");
        assert_eq!(
            report.status.stop_reason().map(StopReason::as_str),
            Some("user")
        );
    }

    #[test]
    fn a_queued_record_is_active_and_a_finished_one_is_not() {
        let queued = record();
        assert!(queued.is_active());
        let finished = finished_record(queued, &ChildTurnReport::completed("ok"), Utc::now());
        assert!(!finished.is_active());
    }

    #[test]
    fn usage_and_tool_counts_survive_the_fold() {
        let mut report = ChildTurnReport::completed("ok");
        report.usage = Usage::new(10, 5);
        report.tool_invocations = 3;
        let finished = finished_record(record(), &report, Utc::now());
        assert_eq!(finished.usage.map(|usage| usage.total_tokens), Some(15));
        assert_eq!(finished.tool_invocations, Some(3));
    }
}
