//! Code workbench projections and durable plan commands.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use ark_contract::{
    FilePreview, GoalRequest, PlanView, RuntimeEvent, TerminalEntriesResponse, TodosRequest,
    WorkspaceChangesResponse,
};

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::service::workbench;
use crate::state::AppState;

async fn events(
    state: &AppState,
    thread: ark_common::ThreadId,
) -> Result<Vec<RuntimeEvent>, ApiError> {
    state.store.get_thread(thread).await?;
    Ok(state.store.replay_events(thread, 0).await?)
}

/// `GET /v1/threads/{id}/plan` and the legacy plan read routes.
pub async fn plan(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PlanView>, ApiError> {
    let thread = parse_thread(&id)?;
    Ok(Json(workbench::plan(&events(&state, thread).await?)))
}

/// `POST /v1/threads/{id}/goal`.
pub async fn update_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<GoalRequest>,
) -> Result<Json<PlanView>, ApiError> {
    let thread = parse_thread(&id)?;
    let goal = body.goal.trim();
    if goal.is_empty() {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            "goal must not be empty",
        ));
    }
    state.store.get_thread(thread).await?;
    state
        .store
        .append_event(
            thread,
            RuntimeEvent::GoalUpdated {
                seq: 0,
                thread_id: thread,
                timestamp: chrono::Utc::now(),
                goal: goal.to_string(),
            },
        )
        .await?;
    Ok(Json(workbench::plan(&events(&state, thread).await?)))
}

/// `DELETE /v1/threads/{id}/goal`.
pub async fn clear_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PlanView>, ApiError> {
    let thread = parse_thread(&id)?;
    state.store.get_thread(thread).await?;
    state
        .store
        .append_event(
            thread,
            RuntimeEvent::GoalCleared {
                seq: 0,
                thread_id: thread,
                timestamp: chrono::Utc::now(),
            },
        )
        .await?;
    Ok(Json(workbench::plan(&events(&state, thread).await?)))
}

/// `POST /v1/threads/{id}/todos`.
pub async fn update_todos(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<TodosRequest>,
) -> Result<Json<PlanView>, ApiError> {
    let thread = parse_thread(&id)?;
    validate_todos(&body.todos)?;
    state.store.get_thread(thread).await?;
    state
        .store
        .append_event(
            thread,
            RuntimeEvent::TodosUpdated {
                seq: 0,
                thread_id: thread,
                timestamp: chrono::Utc::now(),
                todos: body.todos,
            },
        )
        .await?;
    Ok(Json(workbench::plan(&events(&state, thread).await?)))
}

/// `DELETE /v1/threads/{id}/todos`.
pub async fn clear_todos(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PlanView>, ApiError> {
    let thread = parse_thread(&id)?;
    state.store.get_thread(thread).await?;
    state
        .store
        .append_event(
            thread,
            RuntimeEvent::TodosCleared {
                seq: 0,
                thread_id: thread,
                timestamp: chrono::Utc::now(),
            },
        )
        .await?;
    Ok(Json(workbench::plan(&events(&state, thread).await?)))
}

/// `GET /v1/threads/{id}/changes`.
pub async fn changes(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceChangesResponse>, ApiError> {
    let thread = parse_thread(&id)?;
    Ok(Json(workbench::changes(&events(&state, thread).await?)))
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub path: String,
}

/// `GET /v1/threads/{id}/files?path=...`.
pub async fn file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<FilePreview>, ApiError> {
    let thread = parse_thread(&id)?;
    let path = query.path.trim();
    if path.is_empty() {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            "file path must not be empty",
        ));
    }
    workbench::file_preview(&events(&state, thread).await?, path)
        .map(Json)
        .ok_or_else(|| {
            ApiError::new(
                ark_common::ArkErrorCode::NotFound,
                "file has not been read by this thread",
            )
        })
}

/// `GET /v1/threads/{id}/terminal`.
pub async fn terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TerminalEntriesResponse>, ApiError> {
    let thread = parse_thread(&id)?;
    Ok(Json(workbench::terminal(&events(&state, thread).await?)))
}

fn validate_todos(todos: &[ark_contract::TodoEntry]) -> Result<(), ApiError> {
    if todos.len() > 200 {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            "a plan supports at most 200 todos",
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for todo in todos {
        if todo.id.trim().is_empty() || todo.title.trim().is_empty() || !ids.insert(&todo.id) {
            return Err(ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                "todo ids and titles must be non-empty and ids unique",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todos_need_unique_non_empty_ids_and_titles() {
        assert!(validate_todos(&[]).is_ok());
        let duplicate = vec![
            ark_contract::TodoEntry {
                id: "same".into(),
                title: "first".into(),
                status: ark_contract::TodoStatus::Pending,
            },
            ark_contract::TodoEntry {
                id: "same".into(),
                title: "second".into(),
                status: ark_contract::TodoStatus::Pending,
            },
        ];
        assert!(validate_todos(&duplicate).is_err());
    }
}
