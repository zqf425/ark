//! Platform capability routes: providers, skills, workspace, MCP servers.
//!
//! Each handler is a thin pass-through to a port in `state.base`, which is what
//! keeps "what the GUI edits" and "what the runtime uses" the same object. When
//! a build did not wire a service, the route answers a structured `unavailable`
//! instead of pretending the feature exists.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use ark_contract::{
    McpServerEntry, McpServerListResponse, McpServerTestResponse, ProviderEntry,
    ProviderListResponse, ProviderModelCatalogRequest, ProviderModelCatalogResponse,
    ProviderTestRequest, ProviderTestResponse, SkillDiagnostics, SwitchWorkspaceRequest,
    ThreadSkillsResponse, WorkspaceDiff, WorkspaceStatus, WorkspaceView,
};

use crate::api::error::ApiError;
use crate::state::AppState;

fn missing(service: &str) -> ApiError {
    ApiError::new(
        ark_common::ArkErrorCode::RuntimeUnavailable,
        format!("{service} service is not available in this build"),
    )
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

/// `GET /v1/runtime/providers`.
pub async fn providers_list(
    State(state): State<AppState>,
) -> Result<Json<ProviderListResponse>, ApiError> {
    let service = state
        .base
        .providers
        .as_ref()
        .ok_or_else(|| missing("provider"))?;
    Ok(Json(service.list()))
}

/// `POST /v1/runtime/providers` — create or update.
#[derive(Debug, Deserialize)]
pub struct UpsertProviderBody {
    #[serde(flatten)]
    pub entry: ProviderEntry,
    #[serde(default)]
    pub make_default: bool,
}

pub async fn providers_upsert(
    State(state): State<AppState>,
    Json(body): Json<UpsertProviderBody>,
) -> Result<Json<ProviderListResponse>, ApiError> {
    let service = state
        .base
        .providers
        .as_ref()
        .ok_or_else(|| missing("provider"))?;
    Ok(Json(service.upsert(body.entry, body.make_default)?))
}

/// `DELETE /v1/runtime/providers/{id}`.
pub async fn providers_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ProviderListResponse>, ApiError> {
    let service = state
        .base
        .providers
        .as_ref()
        .ok_or_else(|| missing("provider"))?;
    Ok(Json(service.delete(&id)?))
}

/// `POST /v1/runtime/providers/{id}/default`.
pub async fn providers_set_default(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ProviderListResponse>, ApiError> {
    let service = state
        .base
        .providers
        .as_ref()
        .ok_or_else(|| missing("provider"))?;
    Ok(Json(service.set_default(&id)?))
}

/// `POST /v1/runtime/providers/test` — real request, bounded timeout.
pub async fn providers_test(
    State(state): State<AppState>,
    Json(body): Json<ProviderTestRequest>,
) -> Result<Json<ProviderTestResponse>, ApiError> {
    let service = state
        .base
        .providers
        .as_ref()
        .ok_or_else(|| missing("provider"))?;
    Ok(Json(service.test(body).await))
}

/// `POST /v1/runtime/providers/models` — explicit, bounded model discovery.
pub async fn providers_models(
    State(state): State<AppState>,
    Json(body): Json<ProviderModelCatalogRequest>,
) -> Result<Json<ProviderModelCatalogResponse>, ApiError> {
    let service = state
        .base
        .providers
        .as_ref()
        .ok_or_else(|| missing("provider"))?;
    Ok(Json(service.models(body).await?))
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

/// `GET /v1/skills` — runtime-wide base skills only.
pub async fn skills_list(
    State(state): State<AppState>,
) -> Result<Json<SkillDiagnostics>, ApiError> {
    let service = state.base.skills.as_ref().ok_or_else(|| missing("skill"))?;
    Ok(Json(service.list()))
}

/// `GET /v1/threads/{id}/skills`.
///
/// v1 keeps no per-thread extra roots on disk, so the view is the base set plus
/// whatever the store could report. The route exists so the renderer keeps one
/// call shape regardless of when extra roots land.
pub async fn thread_skills(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadSkillsResponse>, ApiError> {
    let service = state.base.skills.as_ref().ok_or_else(|| missing("skill"))?;
    let thread = crate::api::handlers::threads::parse_thread(&id)?;
    state.store.get_thread(thread).await?;
    Ok(Json(service.thread_view(&[])))
}

/// `POST /v1/threads/{id}/skills/refresh` — re-scan, then report.
pub async fn thread_skills_refresh(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadSkillsResponse>, ApiError> {
    let service = state.base.skills.as_ref().ok_or_else(|| missing("skill"))?;
    let thread = crate::api::handlers::threads::parse_thread(&id)?;
    state.store.get_thread(thread).await?;
    service.reload();
    Ok(Json(service.thread_view(&[])))
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct WorkspaceQuery {
    #[serde(default)]
    pub path: Option<String>,
}

/// `GET /v1/workspace/status?path=`.
///
/// No `path` means "no workspace selected": the response is the contract's empty
/// status, which is what the old runtime answered, so the renderer's empty state
/// keeps working.
pub async fn workspace_status(
    State(state): State<AppState>,
    Query(query): Query<WorkspaceQuery>,
) -> Result<Json<WorkspaceStatus>, ApiError> {
    let service = state
        .base
        .workspace
        .as_ref()
        .ok_or_else(|| missing("workspace"))?;
    let raw = query.path.unwrap_or_default();
    Ok(Json(service.status(&raw)))
}

/// `POST /v1/workspace/validate`.
///
/// Validating before storing means a typo in the settings dialog is reported
/// while the user is still looking at the field.
#[derive(Debug, Deserialize)]
pub struct ValidateWorkspaceBody {
    pub path: String,
}

pub async fn workspace_validate(
    State(state): State<AppState>,
    Json(body): Json<ValidateWorkspaceBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let service = state
        .base
        .workspace
        .as_ref()
        .ok_or_else(|| missing("workspace"))?;
    let resolved = service.validate(&body.path)?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "path": resolved.to_string_lossy(),
    })))
}

/// `GET /v1/workspace`.
pub async fn workspace_view(
    State(state): State<AppState>,
) -> Result<Json<WorkspaceView>, ApiError> {
    let service = state
        .base
        .workspace
        .as_ref()
        .ok_or_else(|| missing("workspace"))?;
    Ok(Json(service.view()))
}

/// `POST /v1/workspace/switch`.
///
/// The active count includes both parent turns and delegated child runs. A busy
/// runtime gets a structured 409; no task is cancelled as a side effect.
pub async fn workspace_switch(
    State(state): State<AppState>,
    Json(body): Json<SwitchWorkspaceRequest>,
) -> Result<Json<WorkspaceView>, ApiError> {
    let service = state
        .base
        .workspace
        .as_ref()
        .ok_or_else(|| missing("workspace"))?;
    let _workspace_guard = state.workspace_gate.lock().await;
    let active = state.turns.active() + state.active_children();
    Ok(Json(service.switch(&body.path, active)?))
}

/// `GET /v1/workspace/diff` for the active canonical root only.
pub async fn workspace_diff(
    State(state): State<AppState>,
) -> Result<Json<WorkspaceDiff>, ApiError> {
    let service = state
        .base
        .workspace
        .as_ref()
        .ok_or_else(|| missing("workspace"))?;
    let mut diff = service.diff()?;
    diff.generation = service.view().generation;
    Ok(Json(diff))
}

// ---------------------------------------------------------------------------
// MCP servers
// ---------------------------------------------------------------------------

/// `GET /v1/mcp/servers`.
pub async fn mcp_list(
    State(state): State<AppState>,
) -> Result<Json<McpServerListResponse>, ApiError> {
    let service = state.base.mcp.as_ref().ok_or_else(|| missing("mcp"))?;
    Ok(Json(service.list()))
}

/// `POST /v1/mcp/servers`.
pub async fn mcp_upsert(
    State(state): State<AppState>,
    Json(entry): Json<McpServerEntry>,
) -> Result<Json<McpServerListResponse>, ApiError> {
    let service = state.base.mcp.as_ref().ok_or_else(|| missing("mcp"))?;
    Ok(Json(service.upsert(entry)?))
}

/// `DELETE /v1/mcp/servers/{id}`.
pub async fn mcp_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<McpServerListResponse>, ApiError> {
    let service = state.base.mcp.as_ref().ok_or_else(|| missing("mcp"))?;
    Ok(Json(service.delete(&id)?))
}

/// `POST /v1/mcp/servers/{id}/test` — handshake plus `tools/list`.
pub async fn mcp_test(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<McpServerTestResponse>, ApiError> {
    let service = state.base.mcp.as_ref().ok_or_else(|| missing("mcp"))?;
    Ok(Json(service.test(&id).await))
}
