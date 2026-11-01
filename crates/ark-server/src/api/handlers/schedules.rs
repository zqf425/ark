//! HTTP surface for scheduled tasks (requirements/scheduled-tasks.md §HTTP API).
//!
//! Handlers stay thin: parse, call `ark_schedule` for the domain decisions, call
//! the store for persistence, map the error. Every rule the routes need — the
//! 100-task cap, title/prompt normalization, `next_run_at` — already lives in
//! `ark-schedule`, so this module never re-derives one. A second copy here is
//! how the settings page and the scheduler would start disagreeing.
//!
//! The master switch (`config.schedules.enabled`) gates *triggering*, not
//! editing: a user who turned schedules off can still list, create and fix a
//! task, and the list stays readable. Only `POST /run` answers `409` while it is
//! off — the tick loop reads the same snapshot and idles.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Local;

use ark_contract::{
    CreateScheduleRequest, ScheduledTask, ScheduledTaskListResponse, UpdateScheduleRequest,
};
use ark_schedule::{apply_update, ensure_capacity, prepare_create};

use crate::api::error::ApiError;
use crate::state::AppState;

/// `GET /v1/schedules` — the whole task table.
///
/// Ordering is the store's (most recently updated first), so the settings page
/// shows "what changed lately" without sorting a second time.
pub async fn list(
    State(state): State<AppState>,
) -> Result<Json<ScheduledTaskListResponse>, ApiError> {
    let service = state.schedule_service()?;
    let tasks = service.store().list()?;
    Ok(Json(ScheduledTaskListResponse { tasks }))
}

/// `POST /v1/schedules` — create one task.
///
/// The cap is checked against the live table immediately before the insert, so
/// the answer is the same rule `ensure_capacity` documents even though the two
/// calls are not atomic: a single-user desktop runtime cannot race itself into
/// 101 rows between them.
pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateScheduleRequest>,
) -> Result<(StatusCode, Json<ScheduledTask>), ApiError> {
    let service = state.schedule_service()?;
    let existing = service.store().list()?.len();
    ensure_capacity(existing)?;
    let task = prepare_create(request, existing, Local::now())?;
    let stored = service.store().insert(task)?;
    Ok((StatusCode::CREATED, Json(stored)))
}

/// `PATCH /v1/schedules/{id}` — merge an update onto a stored task.
///
/// `apply_update` recomputes `next_run_at` from "now", so a cadence change and a
/// re-enable both take effect on the next tick without touching the run history.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateScheduleRequest>,
) -> Result<Json<ScheduledTask>, ApiError> {
    let service = state.schedule_service()?;
    let current = service.store().get(&id)?;
    let next = apply_update(&current, request, Local::now())?;
    let stored = service.store().replace(next)?;
    Ok(Json(stored))
}

/// `DELETE /v1/schedules/{id}` — remove a task and its history.
///
/// A missing id is `404` from the store rather than a silent success, so the
/// settings page can tell "already gone" from "delete did nothing".
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let service = state.schedule_service()?;
    service.store().delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/schedules/{id}/run` — run once, now, ignoring the cadence.
///
/// `202` mirrors `POST /v1/threads/{id}/turns`: the thread and the `running`
/// marker exist when the response is written, and the turn itself streams
/// through the thread's own event channel. A task already running is `409`, the
/// same conflict the turn registry reports for a second turn on one thread.
pub async fn run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<ScheduledTask>), ApiError> {
    let service = state.schedule_service()?;
    let task = service.run_now(&state, &id).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::{ScheduleKind, TaskSchedule};

    use crate::service::scheduler::SchedulerService;
    use ark_store::ScheduleFileStore;
    use std::sync::Arc;

    /// A store in a throwaway directory: these tests exercise the handlers, so a
    /// real file store keeps the create→list→patch→delete round trip honest.
    fn service() -> (tempfile::TempDir, Arc<SchedulerService>) {
        let dir = tempfile::Builder::new()
            .prefix("ark-schedule-handlers-")
            .tempdir()
            .expect("temp dir");
        let store = ScheduleFileStore::open(dir.path()).expect("schedule store");
        (dir, Arc::new(SchedulerService::new(Arc::new(store))))
    }

    fn request(prompt: &str) -> CreateScheduleRequest {
        CreateScheduleRequest {
            title: None,
            prompt: prompt.to_string(),
            workspace: None,
            autonomy: None,
            model: None,
            provider_id: None,
            schedule: TaskSchedule {
                kind: ScheduleKind::Interval,
                every_minutes: 30,
                ..TaskSchedule::default()
            },
        }
    }

    /// Call the create path without a router: the handler body is what needs the
    /// assertion, and a `Json` extractor adds nothing to it.
    async fn create_task(
        service: &Arc<SchedulerService>,
        request: CreateScheduleRequest,
    ) -> ScheduledTask {
        let existing = service.store().list().unwrap().len();
        let task = prepare_create(request, existing, Local::now()).expect("prepare create");
        service.store().insert(task).expect("insert")
    }

    #[tokio::test]
    async fn creating_a_task_normalizes_it_and_schedules_the_first_run() {
        let (_dir, service) = service();
        let task = create_task(&service, request("  summarize the inbox  ")).await;

        assert_eq!(task.prompt, "summarize the inbox");
        assert!(!task.id.is_empty(), "the store assigns an id");
        assert!(task.enabled);
        assert!(
            !task.next_run_at.is_empty(),
            "an interval task is scheduled immediately"
        );
    }

    #[tokio::test]
    async fn the_cap_rejects_the_hundred_and_first_task() {
        let (_dir, service) = service();
        for index in 0..ark_contract::MAX_SCHEDULED_TASKS {
            create_task(&service, request(&format!("task {index}"))).await;
        }
        let existing = service.store().list().unwrap().len();
        let error = prepare_create(request("one too many"), existing, Local::now())
            .expect_err("the cap must reject");
        assert_eq!(error.code, ark_common::ArkErrorCode::ValidationError);
        assert_eq!(error.detail.as_deref(), Some("schedule_limit"));
    }

    #[tokio::test]
    async fn an_update_keeps_history_and_recomputes_the_next_run() {
        let (_dir, service) = service();
        let task = create_task(&service, request("nightly report")).await;

        let updated = apply_update(
            &task,
            UpdateScheduleRequest {
                enabled: Some(false),
                ..UpdateScheduleRequest::default()
            },
            Local::now(),
        )
        .expect("apply update");

        assert!(!updated.enabled);
        assert_eq!(updated.next_run_at, "", "a disabled task is not scheduled");
        assert_eq!(updated.id, task.id);
        assert_eq!(updated.created_at, task.created_at);
    }

    #[tokio::test]
    async fn deleting_a_missing_task_is_not_found() {
        let (_dir, service) = service();
        let error = service
            .store()
            .delete("sch_missing")
            .expect_err("must be absent");
        assert_eq!(error.code, ark_common::ArkErrorCode::NotFound);
    }
}
