//! Bounded run history.
//!
//! The history is a small ring kept inside the task document rather than a
//! separate log: the settings page shows "what happened lately" for one task,
//! not an audit trail, and `schedules.json` is rewritten atomically whenever a
//! run finishes. Ten entries is the documented bound; the newest entry is also
//! mirrored into `last_run` so a client can render the current state without
//! walking the list.
//!
//! These are plain functions over `Vec<ScheduledRunRecord>` so the fold is
//! testable without a store, a clock or a runtime.

use ark_contract::{ScheduledRunRecord, ScheduledTask, SCHEDULE_RUN_HISTORY_LIMIT};

/// Trim `history` to the newest [`SCHEDULE_RUN_HISTORY_LIMIT`] entries.
///
/// Ordering is oldest-first, which is the order the settings page reads and the
/// order the wire type documents. Draining from the front keeps the newest tail.
pub fn bound_history(history: &mut Vec<ScheduledRunRecord>) {
    if history.len() > SCHEDULE_RUN_HISTORY_LIMIT {
        let excess = history.len() - SCHEDULE_RUN_HISTORY_LIMIT;
        history.drain(..excess);
    }
}

/// Append one record, dropping the oldest entries past the bound.
///
/// Pure: the caller owns the `Vec`, so the same function serves the store, a
/// test and the scheduler's post-run step.
pub fn push_record(history: &mut Vec<ScheduledRunRecord>, record: ScheduledRunRecord) {
    history.push(record);
    bound_history(history);
}

/// Record one run on a task: append to history and mirror into `last_run`.
///
/// Both writes happen together because the two views must never disagree - a
/// `last_run` newer than the newest history entry would make the UI show a state
/// the log does not contain.
pub fn record_run(task: &mut ScheduledTask, record: ScheduledRunRecord) {
    task.last_run = Some(record.clone());
    push_record(&mut task.history, record);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::{RunStatus, TaskSchedule};
    use chrono::{TimeZone, Utc};

    fn record(offset: u32, status: RunStatus) -> ScheduledRunRecord {
        ScheduledRunRecord {
            status,
            message: format!("run {offset}"),
            thread_id: None,
            started_at: Utc.with_ymd_and_hms(2026, 11, 1, 8, 0, 0).unwrap()
                + chrono::Duration::minutes(i64::from(offset)),
            finished_at: None,
        }
    }

    fn task() -> ScheduledTask {
        ScheduledTask {
            id: "sch_test".to_string(),
            title: "test".to_string(),
            enabled: true,
            prompt: "do it".to_string(),
            workspace: None,
            autonomy: None,
            model: None,
            provider_id: None,
            schedule: TaskSchedule::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_run: None,
            next_run_at: String::new(),
            history: Vec::new(),
        }
    }

    #[test]
    fn history_is_bounded_to_the_newest_entries() {
        let mut history = Vec::new();
        for offset in 0..(SCHEDULE_RUN_HISTORY_LIMIT as u32 + 5) {
            push_record(&mut history, record(offset, RunStatus::Completed));
        }
        assert_eq!(history.len(), SCHEDULE_RUN_HISTORY_LIMIT);
        // The oldest five were dropped; the tail is the newest.
        assert_eq!(history.first().unwrap().message, "run 5");
        assert_eq!(
            history.last().unwrap().message,
            format!("run {}", SCHEDULE_RUN_HISTORY_LIMIT + 4)
        );
    }

    #[test]
    fn bounding_an_already_bounded_history_is_a_no_op() {
        let mut history = vec![
            record(0, RunStatus::Completed),
            record(1, RunStatus::Failed),
        ];
        bound_history(&mut history);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn record_run_keeps_last_run_and_history_consistent() {
        let mut task = task();
        record_run(&mut task, record(1, RunStatus::Running));
        record_run(&mut task, record(2, RunStatus::Completed));

        assert_eq!(task.history.len(), 2);
        let last = task.last_run.as_ref().unwrap();
        assert_eq!(last.status, RunStatus::Completed);
        assert_eq!(last.message, task.history.last().unwrap().message);
    }

    #[test]
    fn skipped_misfires_are_kept_like_any_other_run() {
        let mut task = task();
        for offset in 0..(SCHEDULE_RUN_HISTORY_LIMIT as u32 + 1) {
            record_run(&mut task, record(offset, RunStatus::Skipped));
        }
        assert_eq!(task.history.len(), SCHEDULE_RUN_HISTORY_LIMIT);
        assert_eq!(task.last_run.as_ref().unwrap().status, RunStatus::Skipped);
    }
}
