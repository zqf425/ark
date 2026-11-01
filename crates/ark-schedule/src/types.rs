//! Task shapes plus the one normalization path they all pass through.
//!
//! The wire structs are re-exported from `ark-contract` rather than redefined:
//! the persisted `schedules.json`, the HTTP payload and `web/src/generated` are
//! all generated from that crate, and a second local copy of the shape is
//! exactly how the three drift apart. What lives here is the *policy* over those
//! shapes.
//!
//! Normalization is deliberately repair-oriented (clamp, fall back, truncate)
//! because the alternative - rejecting the whole task - leaves the user with a
//! disabled feature and no hint. The two cases that cannot be repaired
//! silently (an empty prompt, an unusable one-shot time, an unknown autonomy)
//! return a structured error instead.

use ark_common::text::truncate_utf8;
use ark_common::{ArkError, Result};
use ark_contract::MAX_TURN_TEXT_BYTES;
use chrono::{DateTime, Local, Utc};

use crate::next_run::apply_next_run_at;

pub use ark_contract::{
    CreateScheduleRequest, RunStatus, ScheduleKind, ScheduledRunRecord, ScheduledTask,
    TaskSchedule, UpdateScheduleRequest, DEFAULT_SCHEDULE_AUTONOMY, DEFAULT_TIME_OF_DAY,
    MAX_EVERY_MINUTES, MAX_SCHEDULED_TASKS, MIN_EVERY_MINUTES, SCHEDULE_RUN_HISTORY_LIMIT,
    SCHEDULE_TITLE_MAX_BYTES,
};

/// Permission values the runtime accepts for an unattended run.
pub const AUTONOMY_VALUES: [&str; 3] = ["strict", "workspace_write", "unattended"];

/// Placeholder title for a task whose prompt yields no usable first line.
const FALLBACK_TITLE: &str = "Scheduled task";

/// Reject a create that would exceed the task cap.
///
/// The cap lives here because this is the only place that sees both the request
/// and the current task count; the store stays a dumb persistence boundary.
pub fn ensure_capacity(existing: usize) -> Result<()> {
    if existing >= MAX_SCHEDULED_TASKS {
        return Err(ArkError::validation(format!(
            "at most {MAX_SCHEDULED_TASKS} scheduled tasks are supported (already {existing})"
        ))
        .with_detail("schedule_limit"));
    }
    Ok(())
}

/// Parse a strict `HH:mm` wall-clock value.
///
/// Strictness matters: `9:00` and `0900` are what a hand-edited file or a
/// half-written form produces, and quietly accepting them would make the stored
/// value differ from what the user sees. Anything else falls back to 09:00.
pub fn parse_hhmm(raw: &str) -> Option<(u32, u32)> {
    let (hour, minute) = raw.trim().split_once(':')?;
    let digits = |part: &str| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(hour) || !digits(minute) {
        return None;
    }
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

/// Normalize one `HH:mm` value, falling back to [`DEFAULT_TIME_OF_DAY`].
pub fn normalize_hhmm(raw: &str) -> String {
    match parse_hhmm(raw) {
        Some((hour, minute)) => format!("{hour:02}:{minute:02}"),
        None => DEFAULT_TIME_OF_DAY.to_string(),
    }
}

/// Repair every cadence field to a value the scheduler can act on.
///
/// Fields that do not belong to `kind` are still normalized, so flipping a task
/// between daily and weekly never surfaces a stale invalid value.
pub fn normalize_schedule(schedule: &TaskSchedule) -> Result<TaskSchedule> {
    let mut next = schedule.clone();
    next.time_of_day = normalize_hhmm(&schedule.time_of_day);
    next.every_minutes = schedule
        .every_minutes
        .clamp(MIN_EVERY_MINUTES, MAX_EVERY_MINUTES);
    next.day_of_week = normalize_weekdays(&schedule.day_of_week);
    next.day_of_month = normalize_month_days(&schedule.day_of_month);
    next.at_time = match schedule.kind {
        ScheduleKind::At => normalize_at_time(&schedule.at_time)?,
        _ => schedule.at_time.trim().to_string(),
    };
    Ok(next)
}

/// Weekday set, `0` = Sunday; an empty set falls back to Monday (smart-ark).
fn normalize_weekdays(days: &[u8]) -> Vec<u8> {
    let mut kept: Vec<u8> = days.iter().copied().filter(|day| *day <= 6).collect();
    kept.sort_unstable();
    kept.dedup();
    if kept.is_empty() {
        kept.push(1);
    }
    kept
}

/// Day-of-month set; an empty set falls back to the 1st (smart-ark).
fn normalize_month_days(days: &[u8]) -> Vec<u8> {
    let mut kept: Vec<u8> = days
        .iter()
        .copied()
        .filter(|day| (1..=31).contains(day))
        .collect();
    kept.sort_unstable();
    kept.dedup();
    if kept.is_empty() {
        kept.push(1);
    }
    kept
}

/// One-shot instants are stored as UTC RFC3339, so a timezone edit on the
/// machine cannot silently move a task that already fired.
fn normalize_at_time(raw: &str) -> Result<String> {
    let parsed = DateTime::parse_from_rfc3339(raw.trim())
        .map_err(|_| ArkError::validation("`at_time` must be an RFC3339 timestamp"))?;
    Ok(parsed.with_timezone(&Utc).to_rfc3339())
}

/// Trimmed text or `None`, so "cleared" and "absent" stay one state.
fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Normalize an autonomy override; unknown values are rejected rather than
/// silently downgraded to the permissive default.
pub fn normalize_autonomy(raw: Option<&str>) -> Result<Option<String>> {
    let Some(value) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if !AUTONOMY_VALUES.contains(&value) {
        return Err(ArkError::validation(format!(
            "unknown autonomy `{value}`; expected one of {}",
            AUTONOMY_VALUES.join(", ")
        )));
    }
    Ok(Some(value.to_string()))
}

/// Title, truncated to the contract byte budget; an empty title is derived from
/// the prompt so the list never shows a blank row.
pub fn normalize_title(raw: &str, prompt: &str) -> String {
    let trimmed = raw.trim();
    let source = if trimmed.is_empty() { prompt } else { trimmed };
    let first_line = source.lines().next().unwrap_or(source).trim();
    let candidate = truncate_utf8(first_line, SCHEDULE_TITLE_MAX_BYTES).trim();
    if candidate.is_empty() {
        FALLBACK_TITLE.to_string()
    } else {
        candidate.to_string()
    }
}

/// Normalize one task in place-equivalent fashion (takes and returns ownership).
pub fn normalize_task(task: ScheduledTask) -> Result<ScheduledTask> {
    // Copy out of the borrow before the move: `truncate_utf8` returns a slice of
    // the original buffer, so it cannot outlive `task` being destructured.
    let prompt = truncate_utf8(task.prompt.trim(), MAX_TURN_TEXT_BYTES).to_string();
    if prompt.is_empty() {
        return Err(ArkError::validation(
            "a scheduled task needs a non-empty prompt",
        ));
    }
    let mut next = task;
    next.prompt = prompt;
    next.title = normalize_title(&next.title, &next.prompt);
    next.schedule = normalize_schedule(&next.schedule)?;
    next.autonomy = normalize_autonomy(next.autonomy.as_deref())?;
    next.workspace = normalize_optional_text(next.workspace);
    next.model = normalize_optional_text(next.model);
    next.provider_id = normalize_optional_text(next.provider_id);
    next.next_run_at = next.next_run_at.trim().to_string();
    crate::history::bound_history(&mut next.history);
    Ok(next)
}

/// Build a new, enabled task from a create request.
///
/// The id is left empty for the store to fill: id generation needs a UUID
/// source, and the domain crate deliberately has no dependency for it.
pub fn prepare_create(
    request: CreateScheduleRequest,
    existing: usize,
    now: DateTime<Local>,
) -> Result<ScheduledTask> {
    ensure_capacity(existing)?;
    let stamp = now.with_timezone(&Utc);
    let task = ScheduledTask {
        id: String::new(),
        title: request.title.unwrap_or_default(),
        enabled: true,
        prompt: request.prompt,
        workspace: request.workspace,
        autonomy: request.autonomy,
        model: request.model,
        provider_id: request.provider_id,
        schedule: request.schedule,
        created_at: stamp,
        updated_at: stamp,
        last_run: None,
        next_run_at: String::new(),
        history: Vec::new(),
    };
    let mut task = normalize_task(task)?;
    apply_next_run_at(&mut task, now);
    Ok(task)
}

/// Merge a PATCH onto the stored task.
///
/// Absent fields keep their stored value; an empty string clears an optional
/// text field. The merged result passes the same normalization as create, so an
/// update can never store something a create would have repaired.
pub fn apply_update(
    task: &ScheduledTask,
    request: UpdateScheduleRequest,
    now: DateTime<Local>,
) -> Result<ScheduledTask> {
    let mut next = task.clone();
    if let Some(title) = request.title {
        next.title = title;
    }
    if let Some(enabled) = request.enabled {
        next.enabled = enabled;
    }
    if let Some(prompt) = request.prompt {
        next.prompt = prompt;
    }
    if let Some(workspace) = request.workspace {
        next.workspace = Some(workspace);
    }
    if let Some(autonomy) = request.autonomy {
        next.autonomy = Some(autonomy);
    }
    if let Some(model) = request.model {
        next.model = Some(model);
    }
    if let Some(provider_id) = request.provider_id {
        next.provider_id = Some(provider_id);
    }
    if let Some(schedule) = request.schedule {
        next.schedule = schedule;
    }
    next.updated_at = now.with_timezone(&Utc);
    let mut next = normalize_task(next)?;
    // The cadence or the enabled flag may have changed, so the stored fire time
    // is recomputed from "now" instead of reusing the stale one.
    apply_next_run_at(&mut next, now);
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::next_run::parse_stored_next_run;
    use chrono::TimeZone;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 1, 8, 0, 0).unwrap()
    }

    fn draft(kind: ScheduleKind) -> ScheduledTask {
        ScheduledTask {
            id: String::new(),
            title: String::new(),
            enabled: true,
            prompt: "summarize the inbox".to_string(),
            workspace: None,
            autonomy: None,
            model: None,
            provider_id: None,
            schedule: TaskSchedule {
                kind,
                ..TaskSchedule::default()
            },
            created_at: now().with_timezone(&Utc),
            updated_at: now().with_timezone(&Utc),
            last_run: None,
            next_run_at: String::new(),
            history: Vec::new(),
        }
    }

    #[test]
    fn invalid_wall_clock_falls_back_to_0900() {
        for raw in ["", "9:00", "0900", "25:00", "12:60", "aa:bb", "12:5"] {
            assert_eq!(normalize_hhmm(raw), DEFAULT_TIME_OF_DAY, "{raw}");
        }
        assert_eq!(normalize_hhmm(" 07:05 "), "07:05");
    }

    #[test]
    fn empty_weekday_and_month_sets_fall_back() {
        let schedule = normalize_schedule(&TaskSchedule {
            kind: ScheduleKind::Weekly,
            day_of_week: vec![],
            day_of_month: vec![0, 32],
            ..TaskSchedule::default()
        })
        .unwrap();
        assert_eq!(schedule.day_of_week, vec![1]);
        assert_eq!(schedule.day_of_month, vec![1]);

        let deduped = normalize_schedule(&TaskSchedule {
            kind: ScheduleKind::Weekly,
            day_of_week: vec![5, 1, 5, 9],
            day_of_month: vec![30, 1, 30, 31],
            ..TaskSchedule::default()
        })
        .unwrap();
        assert_eq!(deduped.day_of_week, vec![1, 5]);
        assert_eq!(deduped.day_of_month, vec![1, 30, 31]);
    }

    #[test]
    fn interval_is_clamped_to_the_supported_range() {
        let schedule = normalize_schedule(&TaskSchedule {
            kind: ScheduleKind::Interval,
            every_minutes: 0,
            ..TaskSchedule::default()
        })
        .unwrap();
        assert_eq!(schedule.every_minutes, MIN_EVERY_MINUTES);

        let huge = normalize_schedule(&TaskSchedule {
            kind: ScheduleKind::Interval,
            every_minutes: 9_999_999,
            ..TaskSchedule::default()
        })
        .unwrap();
        assert_eq!(huge.every_minutes, MAX_EVERY_MINUTES);
    }

    #[test]
    fn title_is_derived_then_truncated_on_char_boundaries() {
        let task = normalize_task(ScheduledTask {
            prompt: "first line\nsecond line".to_string(),
            ..draft(ScheduleKind::Manual)
        })
        .unwrap();
        assert_eq!(task.title, "first line");

        let long = "x".repeat(SCHEDULE_TITLE_MAX_BYTES + 50);
        let truncated = normalize_task(ScheduledTask {
            title: long,
            ..draft(ScheduleKind::Manual)
        })
        .unwrap();
        assert_eq!(truncated.title.len(), SCHEDULE_TITLE_MAX_BYTES);

        let multibyte = "你".repeat(SCHEDULE_TITLE_MAX_BYTES);
        let cut = normalize_task(ScheduledTask {
            title: multibyte,
            ..draft(ScheduleKind::Manual)
        })
        .unwrap();
        assert!(cut.title.len() <= SCHEDULE_TITLE_MAX_BYTES);
        assert!(cut.title.chars().all(|ch| ch == '你'));
    }

    #[test]
    fn prompt_is_truncated_to_the_turn_limit() {
        let prompt = "y".repeat(MAX_TURN_TEXT_BYTES + 10);
        let task = normalize_task(ScheduledTask {
            prompt,
            ..draft(ScheduleKind::Manual)
        })
        .unwrap();
        assert_eq!(task.prompt.len(), MAX_TURN_TEXT_BYTES);
    }

    #[test]
    fn empty_prompt_and_unknown_autonomy_are_structured_errors() {
        let empty = normalize_task(ScheduledTask {
            prompt: "   ".to_string(),
            ..draft(ScheduleKind::Manual)
        })
        .unwrap_err();
        assert_eq!(empty.code, ark_common::ArkErrorCode::ValidationError);

        let unknown = normalize_task(ScheduledTask {
            autonomy: Some("god_mode".to_string()),
            ..draft(ScheduleKind::Manual)
        })
        .unwrap_err();
        assert!(unknown.message.contains("god_mode"));
    }

    #[test]
    fn clearing_optional_text_folds_to_none() {
        let task = normalize_task(ScheduledTask {
            workspace: Some("  ".to_string()),
            model: Some(" model ".to_string()),
            autonomy: Some(" workspace_write ".to_string()),
            ..draft(ScheduleKind::Manual)
        })
        .unwrap();
        assert_eq!(task.workspace, None);
        assert_eq!(task.model.as_deref(), Some("model"));
        assert_eq!(task.autonomy.as_deref(), Some("workspace_write"));
    }

    #[test]
    fn one_shot_time_is_required_and_stored_as_utc() {
        let empty = normalize_schedule(&TaskSchedule {
            kind: ScheduleKind::At,
            at_time: String::new(),
            ..TaskSchedule::default()
        });
        assert!(empty.is_err());

        let normalized = normalize_schedule(&TaskSchedule {
            kind: ScheduleKind::At,
            at_time: "2026-11-05T09:30:00+08:00".to_string(),
            ..TaskSchedule::default()
        })
        .unwrap();
        assert!(
            normalized.at_time.ends_with("+00:00"),
            "{}",
            normalized.at_time
        );
    }

    #[test]
    fn create_enforces_the_task_cap() {
        let request = CreateScheduleRequest {
            title: None,
            prompt: "run the report".to_string(),
            workspace: None,
            autonomy: None,
            model: None,
            provider_id: None,
            schedule: TaskSchedule {
                kind: ScheduleKind::Daily,
                time_of_day: "08:30".to_string(),
                ..TaskSchedule::default()
            },
        };
        let task = prepare_create(request.clone(), MAX_SCHEDULED_TASKS - 1, now()).unwrap();
        assert!(task.enabled);
        assert!(parse_stored_next_run(&task.next_run_at).is_some());

        let rejected = prepare_create(request, MAX_SCHEDULED_TASKS, now()).unwrap_err();
        assert_eq!(rejected.code, ark_common::ArkErrorCode::ValidationError);
        assert_eq!(rejected.detail.as_deref(), Some("schedule_limit"));
    }

    #[test]
    fn update_merges_and_recomputes_the_next_run() {
        let created = prepare_create(
            CreateScheduleRequest {
                title: Some("daily digest".to_string()),
                prompt: "summarize".to_string(),
                workspace: None,
                autonomy: None,
                model: None,
                provider_id: None,
                schedule: TaskSchedule {
                    kind: ScheduleKind::Daily,
                    time_of_day: "09:00".to_string(),
                    ..TaskSchedule::default()
                },
            },
            0,
            now(),
        )
        .unwrap();

        let paused = apply_update(
            &created,
            UpdateScheduleRequest {
                enabled: Some(false),
                ..UpdateScheduleRequest::default()
            },
            now(),
        )
        .unwrap();
        assert!(!paused.enabled);
        assert_eq!(paused.next_run_at, "");
        assert_eq!(paused.title, "daily digest");

        let resumed = apply_update(
            &paused,
            UpdateScheduleRequest {
                enabled: Some(true),
                workspace: Some(String::new()),
                ..UpdateScheduleRequest::default()
            },
            now(),
        )
        .unwrap();
        assert!(resumed.enabled);
        assert_ne!(resumed.next_run_at, "");
        assert_eq!(resumed.workspace, None);
        assert_eq!(resumed.created_at, created.created_at);
    }
}
