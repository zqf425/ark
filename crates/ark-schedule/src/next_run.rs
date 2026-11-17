//! The single place that decides when a task fires next.
//!
//! Two properties make this module small on purpose:
//!
//! * `from` is always a parameter. The scheduler passes the wall clock, tests
//!   pass a fixed instant, and nothing here calls `Local::now()`.
//! * The stored wire value is a plain string with `""` meaning "not scheduled".
//!   One sentinel for manual/disabled/already-fired keeps the renderer from
//!   having to distinguish three flavours of `null`.
//!
//! Daily/weekly/monthly scan forward day by day in **local** time rather than
//! adding 24-hour increments. A DST transition makes a "day" 23 or 25 hours
//! long, and adding a fixed offset would drift a 09:00 task to 08:00 or 10:00
//! twice a year; walking calendar dates keeps the wall-clock time stable.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};

use crate::types::parse_hhmm;
use crate::types::{ScheduleKind, ScheduledTask};

/// How far the monthly scan will look for a matching day of month.
///
/// 366 days is a hard stop rather than a year of attempts: February 30th never
/// occurs, and a task configured that way must terminate the search instead of
/// spinning. All real dates are found well inside this window.
const MONTHLY_SCAN_LIMIT_DAYS: i64 = 366;

/// How far the weekly scan will look. A valid weekday set always matches within
/// 7 days; the 8th day is the inclusive boundary check.
const WEEKLY_SCAN_LIMIT_DAYS: i64 = 8;

/// Compute the next fire time, or `None` when the task is not scheduled.
///
/// `None` covers exactly the documented cases: a manual task, a disabled task,
/// and a one-shot `at` task whose instant is not in the future. Callers turn
/// `None` into the empty string on the wire.
pub fn compute_next_run_at(task: &ScheduledTask, from: DateTime<Local>) -> Option<DateTime<Local>> {
    if !task.enabled {
        return None;
    }
    match task.schedule.kind {
        ScheduleKind::Manual => None,
        ScheduleKind::At => parse_at(&task.schedule.at_time)
            .filter(|instant| *instant > from)
            .map(|instant| instant.with_timezone(&Local)),
        ScheduleKind::Interval => {
            // Counted from the completion instant: a run
            // that took 10 minutes does not fire again 5 minutes later.
            let minutes = i64::from(task.schedule.every_minutes.max(1));
            Some(from + Duration::minutes(minutes))
        }
        ScheduleKind::Daily => next_daily(task, from),
        ScheduleKind::Weekly => next_weekly(task, from),
        ScheduleKind::Monthly => next_monthly(task, from),
    }
}

/// Whether a task is due at `now`; `false` when it has no scheduled time.
///
/// An empty or unparsable stored timestamp reads as "not scheduled" instead of
/// "due now": a corrupted file must not fire every task at startup.
pub fn is_due(task: &ScheduledTask, now: DateTime<Local>) -> bool {
    stored_instant(task).is_some_and(|instant| instant <= now)
}

/// Rewrite `task.next_run_at` from `now`, returning the value written.
///
/// `next_run_at` is the persisted decision, so it is refreshed in exactly the
/// two moments the requirement names: after a cadence change and after a run
/// finishes.
pub fn apply_next_run_at(task: &mut ScheduledTask, now: DateTime<Local>) -> String {
    task.next_run_at = format_next_run_at(compute_next_run_at(task, now));
    task.next_run_at.clone()
}

/// Wire form of an optional next run: RFC3339 local time, or `""`.
pub fn format_next_run_at(next: Option<DateTime<Local>>) -> String {
    next.map(|instant| instant.to_rfc3339()).unwrap_or_default()
}

/// Parse the stored timestamp, treating `""` and garbage as "not scheduled".
pub fn parse_stored_next_run(raw: &str) -> Option<DateTime<Local>> {
    if raw.trim().is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|instant| instant.with_timezone(&Local))
}

fn stored_instant(task: &ScheduledTask) -> Option<DateTime<Local>> {
    parse_stored_next_run(&task.next_run_at)
}

/// Seconds since local midnight, from the normalized `HH:mm` field.
fn time_of_day(task: &ScheduledTask) -> (u32, u32) {
    parse_hhmm(&task.schedule.time_of_day).unwrap_or((9, 0))
}

/// The wall-clock instant for `date` at the task's time of day.
///
/// A DST spring-forward gap has no such local time. Chrono reports it as
/// ambiguous/absent; the earliest valid instant of that day is used so the task
/// still fires once instead of being silently dropped for a day.
fn at_local_time(date: NaiveDate, hour: u32, minute: u32) -> Option<DateTime<Local>> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(instant) => Some(instant),
        chrono::LocalResult::Ambiguous(earliest, _) => Some(earliest),
        chrono::LocalResult::None => Local
            .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
            .earliest(),
    }
}

fn next_daily(task: &ScheduledTask, from: DateTime<Local>) -> Option<DateTime<Local>> {
    let (hour, minute) = time_of_day(task);
    let mut date = from.date_naive();
    // Today first: a 09:00 task asked at 08:00 fires today, not tomorrow.
    for _ in 0..=1 {
        let candidate = at_local_time(date, hour, minute)?;
        if candidate > from {
            return Some(candidate);
        }
        date = date.succ_opt()?;
    }
    None
}

fn next_weekly(task: &ScheduledTask, from: DateTime<Local>) -> Option<DateTime<Local>> {
    let (hour, minute) = time_of_day(task);
    let weekdays = &task.schedule.day_of_week;
    let start = from.date_naive();
    for offset in 0..WEEKLY_SCAN_LIMIT_DAYS {
        let date = start.checked_add_signed(Duration::days(offset))?;
        // chrono numbers Sunday as 0 via `num_days_from_sunday`, which is the
        // documented wire convention for `day_of_week`.
        if !weekdays.contains(&(date.weekday().num_days_from_sunday() as u8)) {
            continue;
        }
        let candidate = at_local_time(date, hour, minute)?;
        if candidate > from {
            return Some(candidate);
        }
    }
    None
}

fn next_monthly(task: &ScheduledTask, from: DateTime<Local>) -> Option<DateTime<Local>> {
    let (hour, minute) = time_of_day(task);
    let days = &task.schedule.day_of_month;
    let start = from.date_naive();
    for offset in 0..MONTHLY_SCAN_LIMIT_DAYS {
        let date = start.checked_add_signed(Duration::days(offset))?;
        if !days.contains(&(date.day() as u8)) {
            continue;
        }
        let candidate = at_local_time(date, hour, minute)?;
        if candidate > from {
            return Some(candidate);
        }
    }
    None
}

fn parse_at(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|instant| instant.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TaskSchedule, DEFAULT_TIME_OF_DAY};

    fn at_local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("unambiguous local instant")
    }

    fn task(kind: ScheduleKind) -> ScheduledTask {
        ScheduledTask {
            id: "sch_test".to_string(),
            title: "test".to_string(),
            enabled: true,
            prompt: "do it".to_string(),
            workspace: None,
            autonomy: None,
            model: None,
            provider_id: None,
            schedule: TaskSchedule {
                kind,
                every_minutes: 30,
                time_of_day: DEFAULT_TIME_OF_DAY.to_string(),
                at_time: String::new(),
                day_of_week: Vec::new(),
                day_of_month: Vec::new(),
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_run: None,
            next_run_at: String::new(),
            history: Vec::new(),
        }
    }

    #[test]
    fn manual_and_disabled_tasks_are_never_scheduled() {
        let now = at_local(2026, 11, 1, 8, 0);
        assert!(compute_next_run_at(&task(ScheduleKind::Manual), now).is_none());

        let mut disabled = task(ScheduleKind::Daily);
        disabled.enabled = false;
        assert!(compute_next_run_at(&disabled, now).is_none());
    }

    #[test]
    fn interval_counts_from_the_completion_instant() {
        let now = at_local(2026, 11, 1, 8, 0);
        let next = compute_next_run_at(&task(ScheduleKind::Interval), now).unwrap();
        assert_eq!(next, now + Duration::minutes(30));

        // Crossing midnight is ordinary arithmetic for an interval task.
        let late = at_local(2026, 11, 1, 23, 45);
        let next = compute_next_run_at(&task(ScheduleKind::Interval), late).unwrap();
        assert_eq!(next, at_local(2026, 11, 2, 0, 15));
    }

    #[test]
    fn interval_clamps_a_zero_cadence_instead_of_looping() {
        let mut zero = task(ScheduleKind::Interval);
        zero.schedule.every_minutes = 0;
        let now = at_local(2026, 11, 1, 8, 0);
        assert_eq!(
            compute_next_run_at(&zero, now).unwrap(),
            now + Duration::minutes(1)
        );
    }

    #[test]
    fn one_shot_fires_once_then_never_again() {
        let mut once = task(ScheduleKind::At);
        once.schedule.at_time = at_local(2026, 11, 5, 9, 30).to_rfc3339();

        let before = at_local(2026, 11, 1, 8, 0);
        assert_eq!(
            compute_next_run_at(&once, before).unwrap(),
            at_local(2026, 11, 5, 9, 30)
        );
        assert!(!is_due(&once, before));

        // One second past the instant the task is spent: no catch-up firing.
        let after = at_local(2026, 11, 5, 9, 30) + Duration::seconds(1);
        assert!(compute_next_run_at(&once, after).is_none());

        let exactly = at_local(2026, 11, 5, 9, 30);
        assert!(compute_next_run_at(&once, exactly).is_none());
    }

    #[test]
    fn daily_uses_today_when_the_time_is_still_ahead() {
        let mut daily = task(ScheduleKind::Daily);
        daily.schedule.time_of_day = "09:00".to_string();

        let early = at_local(2026, 11, 1, 8, 0);
        assert_eq!(
            compute_next_run_at(&daily, early).unwrap(),
            at_local(2026, 11, 1, 9, 0)
        );

        // At the exact fire time the task has fired: the next occurrence is
        // tomorrow, so a tick cannot fire the same slot twice.
        let exact = at_local(2026, 11, 1, 9, 0);
        assert_eq!(
            compute_next_run_at(&daily, exact).unwrap(),
            at_local(2026, 11, 2, 9, 0)
        );
    }

    #[test]
    fn daily_crosses_a_month_boundary() {
        let mut daily = task(ScheduleKind::Daily);
        daily.schedule.time_of_day = "09:00".to_string();
        let end_of_month = at_local(2026, 11, 30, 10, 0);
        assert_eq!(
            compute_next_run_at(&daily, end_of_month).unwrap(),
            at_local(2026, 12, 1, 9, 0)
        );
    }

    #[test]
    fn invalid_time_of_day_falls_back_to_nine() {
        let mut daily = task(ScheduleKind::Daily);
        daily.schedule.time_of_day = "25:99".to_string();
        let now = at_local(2026, 11, 1, 8, 0);
        assert_eq!(
            compute_next_run_at(&daily, now).unwrap(),
            at_local(2026, 11, 1, 9, 0)
        );
    }

    #[test]
    fn weekly_scans_to_the_next_selected_weekday() {
        let mut weekly = task(ScheduleKind::Weekly);
        weekly.schedule.time_of_day = "09:00".to_string();
        // 2026-11-01 is a Sunday, so [1] (Monday) is tomorrow.
        weekly.schedule.day_of_week = vec![1];
        let sunday = at_local(2026, 11, 1, 8, 0);
        assert_eq!(
            compute_next_run_at(&weekly, sunday).unwrap(),
            at_local(2026, 11, 2, 9, 0)
        );

        // A second selected day later in the week is preferred when today has
        // already passed its time.
        weekly.schedule.day_of_week = vec![0, 3];
        let sunday_late = at_local(2026, 11, 1, 10, 0);
        assert_eq!(
            compute_next_run_at(&weekly, sunday_late).unwrap(),
            at_local(2026, 11, 4, 9, 0)
        );
    }

    #[test]
    fn weekly_honours_a_later_time_on_the_same_day() {
        let mut weekly = task(ScheduleKind::Weekly);
        weekly.schedule.time_of_day = "18:00".to_string();
        weekly.schedule.day_of_week = vec![0];
        let sunday_morning = at_local(2026, 11, 1, 8, 0);
        assert_eq!(
            compute_next_run_at(&weekly, sunday_morning).unwrap(),
            at_local(2026, 11, 1, 18, 0)
        );
    }

    #[test]
    fn weekly_stops_at_the_scan_limit_when_no_day_matches() {
        let mut broken = task(ScheduleKind::Weekly);
        broken.schedule.day_of_week = vec![9];
        let now = at_local(2026, 11, 1, 8, 0);
        assert!(compute_next_run_at(&broken, now).is_none());
    }

    #[test]
    fn monthly_fires_on_the_configured_day_and_crosses_the_month() {
        let mut monthly = task(ScheduleKind::Monthly);
        monthly.schedule.time_of_day = "07:15".to_string();
        monthly.schedule.day_of_month = vec![15];

        let now = at_local(2026, 11, 1, 8, 0);
        assert_eq!(
            compute_next_run_at(&monthly, now).unwrap(),
            at_local(2026, 11, 15, 7, 15)
        );

        let after = at_local(2026, 11, 15, 8, 0);
        assert_eq!(
            compute_next_run_at(&monthly, after).unwrap(),
            at_local(2026, 12, 15, 7, 15)
        );
    }

    #[test]
    fn monthly_skips_months_without_the_day() {
        let mut monthly = task(ScheduleKind::Monthly);
        monthly.schedule.time_of_day = "07:15".to_string();
        monthly.schedule.day_of_month = vec![31];

        // November has 30 days, so a day-31 task jumps to December 31st.
        let now = at_local(2026, 11, 1, 8, 0);
        assert_eq!(
            compute_next_run_at(&monthly, now).unwrap(),
            at_local(2026, 12, 31, 7, 15)
        );
    }

    #[test]
    fn monthly_terminates_for_an_impossible_day() {
        let mut monthly = task(ScheduleKind::Monthly);
        // 32 is out of range and never matches, proving the scan is bounded.
        monthly.schedule.day_of_month = vec![32];
        let now = at_local(2026, 11, 1, 8, 0);
        assert!(compute_next_run_at(&monthly, now).is_none());
    }

    #[test]
    fn due_requires_a_parsable_stored_instant() {
        let mut daily = task(ScheduleKind::Daily);
        let now = at_local(2026, 11, 1, 9, 0);

        assert!(!is_due(&daily, now), "empty means not scheduled");

        daily.next_run_at = "not-a-time".to_string();
        assert!(!is_due(&daily, now), "garbage must not fire");

        daily.next_run_at = format_next_run_at(Some(now));
        assert!(is_due(&daily, now));

        daily.next_run_at = format_next_run_at(Some(now + Duration::minutes(1)));
        assert!(!is_due(&daily, now));
    }

    #[test]
    fn stored_timestamp_round_trips_through_the_wire_string() {
        let mut daily = task(ScheduleKind::Daily);
        let now = at_local(2026, 11, 1, 8, 0);
        let written = apply_next_run_at(&mut daily, now);
        assert_eq!(written, daily.next_run_at);
        assert_eq!(
            parse_stored_next_run(&written),
            Some(at_local(2026, 11, 1, 9, 0))
        );

        daily.enabled = false;
        apply_next_run_at(&mut daily, now);
        assert_eq!(daily.next_run_at, "");
    }
}
