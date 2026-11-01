//! Wire types for scheduled tasks (docs/requirements/scheduled-tasks.md).
//!
//! These shapes are the single source for the persisted `schedules.json`
//! document, the HTTP payloads and the generated TypeScript. They deliberately
//! carry no scheduling logic: normalization and `next_run_at` live in
//! `ark-schedule`, so a second caller (store, server, tests) cannot invent a
//! different rule set.
//!
//! `next_run_at` is a timestamp string rather than an `Option`: the approved
//! requirement uses the empty string as "not scheduled" (manual, disabled, or a
//! one-shot `at` task that already fired), and keeping that one sentinel avoids
//! two representations of the same state on the wire.

use ark_common::ThreadId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum number of tasks the store accepts.
///
/// Exported so the settings UI can disable "create" at the same boundary the
/// runtime enforces, instead of hard-coding a second copy of the number.
pub const MAX_SCHEDULED_TASKS: usize = 100;

/// Maximum UTF-8 byte length of a task title.
pub const SCHEDULE_TITLE_MAX_BYTES: usize = 200;

/// Retained run records per task (oldest are dropped first).
pub const SCHEDULE_RUN_HISTORY_LIMIT: usize = 10;

/// Lower bound of `every_minutes`.
pub const MIN_EVERY_MINUTES: u32 = 1;

/// Upper bound of `every_minutes`: one year.
pub const MAX_EVERY_MINUTES: u32 = 525_600;

/// Fallback `time_of_day` when the supplied value is not a valid `HH:mm`.
pub const DEFAULT_TIME_OF_DAY: &str = "09:00";

/// Autonomy applied to unattended scheduled runs when the task names none.
pub const DEFAULT_SCHEDULE_AUTONOMY: &str = "unattended";

/// Trigger cadence of one task, aligned with smart-ark's five kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    #[default]
    Manual,
    At,
    Interval,
    Daily,
    Weekly,
    Monthly,
}

/// Terminal state of one scheduled run.
///
/// Names follow the runtime's event vocabulary (`turn_completed`,
/// `turn_failed`, `turn_aborted`) so a schedule row and a thread row describe
/// the same outcome with the same word. `Skipped` is the misfire marker and
/// `Running` is written when a tick starts the turn.
// No `Default`: "the default run outcome" has no meaning, and a derive would
// quietly pick one terminal state for a record that never ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Aborted,
    Skipped,
}

/// The five cadences, flattened into one struct.
///
/// Fields that do not belong to `kind` are ignored but preserved, so a user can
/// switch a task between daily and weekly without losing the other settings.
/// Normalization (in `ark-schedule`) is what makes the relevant fields valid.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct TaskSchedule {
    pub kind: ScheduleKind,
    /// Interval cadence, clamped to `[MIN_EVERY_MINUTES, MAX_EVERY_MINUTES]`.
    #[serde(default)]
    pub every_minutes: u32,
    /// Local wall-clock time for daily/weekly/monthly, normalized to `HH:mm`.
    #[serde(default)]
    pub time_of_day: String,
    /// RFC3339 instant for a one-shot task; empty for every other kind.
    #[serde(default)]
    pub at_time: String,
    /// Weekly weekdays, `0` = Sunday; normalized to a sorted, deduplicated set.
    #[serde(default)]
    pub day_of_week: Vec<u8>,
    /// Monthly days of month (`1..=31`); normalized to a sorted set.
    #[serde(default)]
    pub day_of_month: Vec<u8>,
}

/// One finished (or in-flight) scheduled run, kept in the bounded history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ScheduledRunRecord {
    pub status: RunStatus,
    /// Short user-facing summary: the assistant's last text, or the error.
    #[serde(default)]
    pub message: String,
    /// Thread the run created, so the UI can open the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}

/// One scheduled task: prompt, cadence, and its last known run state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ScheduledTask {
    /// `sch_<uuid>`; assigned by the store when a create request omits it.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// Missing flag reads as disabled: a damaged or older document must not
    /// start firing unattended turns.
    #[serde(default)]
    pub enabled: bool,
    /// User message sent for every run.
    #[serde(default)]
    pub prompt: String,
    /// Empty means the runtime's default workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// `strict` | `workspace_write` | `unattended`; empty falls back to
    /// `DEFAULT_SCHEDULE_AUTONOMY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
    /// Empty means the runtime's default provider/model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub schedule: TaskSchedule,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<ScheduledRunRecord>,
    /// Empty string = not scheduled (manual, disabled, or a fired one-shot).
    #[serde(default)]
    pub next_run_at: String,
    /// Bounded history, oldest first, capped at `SCHEDULE_RUN_HISTORY_LIMIT`.
    #[serde(default)]
    pub history: Vec<ScheduledRunRecord>,
}

/// `GET /v1/schedules` — the whole task table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ScheduledTaskListResponse {
    #[serde(default)]
    pub tasks: Vec<ScheduledTask>,
}

/// `POST /v1/schedules`.
///
/// Only `prompt` and `schedule` are required; an absent title is derived from
/// the prompt by `ark-schedule::prepare_create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct CreateScheduleRequest {
    #[serde(default)]
    pub title: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub autonomy: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    pub schedule: TaskSchedule,
}

/// `PATCH /v1/schedules/{id}`.
///
/// Every field is a merge, never a replacement: an absent field keeps the
/// stored value. Clearing an optional text field (`workspace`, `model`,
/// `provider_id`, `autonomy`) is expressed by sending an empty string, which
/// normalization folds back to "unset".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct UpdateScheduleRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub autonomy: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub schedule: Option<TaskSchedule>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_status_use_the_event_vocabulary() {
        assert_eq!(
            serde_json::to_string(&ScheduleKind::Weekly).unwrap(),
            "\"weekly\""
        );
        assert_eq!(
            serde_json::to_string(&RunStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&RunStatus::Aborted).unwrap(),
            "\"aborted\""
        );
    }

    #[test]
    fn absent_enabled_flag_reads_as_disabled() {
        let task: ScheduledTask = serde_json::from_str(
            r#"{
                "id": "sch_1",
                "title": "t",
                "prompt": "p",
                "schedule": { "kind": "daily", "time_of_day": "09:00" },
                "created_at": "2026-11-01T00:00:00Z",
                "updated_at": "2026-11-01T00:00:00Z"
            }"#,
        )
        .unwrap();
        assert!(!task.enabled);
        assert_eq!(task.next_run_at, "");
        assert!(task.history.is_empty());
    }
}
