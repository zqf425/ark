//! Platform capability routes: providers, skills, workspace, MCP servers.
//!
//! Each handler is a thin pass-through to a port in `state.base`, which is what
//! keeps "what the GUI edits" and "what the runtime uses" the same object. When
//! a build did not wire a service, the route answers a structured `unavailable`
//! instead of pretending the feature exists.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use ark_contract::{
    FolderPickResponse, McpServerEntry, McpServerListResponse, McpServerTestResponse,
    PluginEnableRequest, PluginHostReport, PluginProvisionView, ProviderEntry,
    ProviderListResponse, ProviderModelCatalogRequest, ProviderModelCatalogResponse,
    ProviderTestRequest, ProviderTestResponse, SkillDiagnostics, SwitchWorkspaceRequest,
    ThreadSkillsResponse, WorkspaceDiff, WorkspaceFilePreview, WorkspaceStatus,
    WorkspaceValidateResponse, WorkspaceView,
};
use chrono::Utc;

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

/// `GET /v1/runtime/providers/{id}` — the full editable entry, including the
/// stored api key. Only the settings edit form calls this; the list stays redacted.
pub async fn providers_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ProviderEntry>, ApiError> {
    let service = state
        .base
        .providers
        .as_ref()
        .ok_or_else(|| missing("provider"))?;
    Ok(Json(service.get(&id)?))
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
///
/// The optional body pins one model of that provider; a missing body (or a
/// blank model) keeps the provider's stored `model` as its default.
#[derive(Debug, Default, Deserialize)]
pub struct SetDefaultProviderBody {
    #[serde(default)]
    pub model: Option<String>,
}

pub async fn providers_set_default(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<SetDefaultProviderBody>>,
) -> Result<Json<ProviderListResponse>, ApiError> {
    let service = state
        .base
        .providers
        .as_ref()
        .ok_or_else(|| missing("provider"))?;
    let model = body.and_then(|Json(body)| body.model);
    Ok(Json(service.set_default(&id, model.as_deref())?))
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

/// Project candidates from the thread index: every distinct thread workspace
/// with its newest activity. The index is derived, in-memory state, so one
/// `list_threads` per workspace read is cheap; it is what the session rail
/// already reads its rows from.
async fn project_candidates(
    state: &AppState,
) -> Vec<crate::service::workspace_projects::ProjectCandidate> {
    match state.store.list_threads().await {
        Ok(metas) => {
            let mut newest: HashMap<String, chrono::DateTime<Utc>> = HashMap::new();
            for meta in metas {
                let Some(workspace) = meta.runtime.workspace.as_deref().map(str::trim) else {
                    continue;
                };
                if workspace.is_empty() {
                    continue;
                }
                let at = meta.last_event_at.unwrap_or(meta.updated_at);
                newest
                    .entry(workspace.to_string())
                    .and_modify(|slot| *slot = (*slot).max(at))
                    .or_insert(at);
            }
            newest
                .into_iter()
                .map(
                    |(path, last_event_at)| crate::service::workspace_projects::ProjectCandidate {
                        path,
                        active: false,
                        last_event_at: Some(last_event_at),
                    },
                )
                .collect()
        }
        // The project menu is display-only derived data; an index hiccup must
        // not hide the active/recent roots the `WorkspaceRoots` view owns.
        Err(error) => {
            tracing::warn!(error = %error, "could not read thread workspaces for the project view");
            Vec::new()
        }
    }
}

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
) -> Result<Json<WorkspaceValidateResponse>, ApiError> {
    let service = state
        .base
        .workspace
        .as_ref()
        .ok_or_else(|| missing("workspace"))?;
    let resolved = service.validate(&body.path)?;
    Ok(Json(WorkspaceValidateResponse {
        ok: true,
        path: ark_common::path::normalize(&resolved.to_string_lossy()),
    }))
}

/// `POST /v1/workspace/pick` — open the host's native folder chooser.
///
/// The dialog is modal on the host machine, so the blocking picker runs in
/// `spawn_blocking`; cancellation of the HTTP request does not close an
/// already-open dialog. `path` is `None` when the user cancelled.
pub async fn workspace_pick(
    State(state): State<AppState>,
) -> Result<Json<FolderPickResponse>, ApiError> {
    let service = state
        .base
        .folder_dialog
        .as_ref()
        .cloned()
        .ok_or_else(|| missing("folder dialog"))?;
    let picked = tokio::task::spawn_blocking(move || service.pick_folder())
        .await
        .map_err(|error| {
            ApiError::new(
                ark_common::ArkErrorCode::InternalError,
                format!("folder dialog task failed: {error}"),
            )
        })?;
    Ok(Json(FolderPickResponse {
        path: picked.map(|path| path.to_string_lossy().into_owned()),
    }))
}

/// `GET /v1/workspace`.
///
/// The returned `recent` list is the merged project menu: the process recent
/// history plus every distinct thread workspace from the index, so a session
/// that was only ever switched at the thread level (the composer's pick) is
/// still offered here. `active` stays the host's canonical root.
pub async fn workspace_view(
    State(state): State<AppState>,
) -> Result<Json<WorkspaceView>, ApiError> {
    let service = state
        .base
        .workspace
        .as_ref()
        .ok_or_else(|| missing("workspace"))?;
    let candidates = project_candidates(&state).await;
    Ok(Json(
        crate::service::workspace_projects::merge_workspace_projects(service.view(), candidates),
    ))
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
        .cloned()
        .ok_or_else(|| missing("workspace"))?;
    let switch_state = state.clone();
    let view = tokio::task::spawn_blocking(move || {
        // The blocking task owns the gate so request cancellation cannot release
        // it before path validation, state publication, and persistence finish.
        let _workspace_guard = switch_state.workspace_gate.blocking_lock();
        let active = switch_state.turns.active() + switch_state.active_children();
        service.switch(&body.path, active)
    })
    .await
    .map_err(|error| {
        ApiError::new(
            ark_common::ArkErrorCode::RuntimeUnavailable,
            format!("workspace switch task failed: {error}"),
        )
    })??;
    // Same merged projection as `workspace_view`: a switch response is also a
    // project-menu refresh for the settings surface.
    let candidates = project_candidates(&state).await;
    Ok(Json(
        crate::service::workspace_projects::merge_workspace_projects(view, candidates),
    ))
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

/// Query parameters of `GET /v1/workspace/file?path=...`.
#[derive(Debug, Deserialize)]
pub struct WorkspaceFileQuery {
    pub path: String,
    /// Session whose effective workspace authorizes this read
    /// (requirements/workspace-file-preview.md). Absent keeps the process-wide
    /// active root, which is what the settings and review surfaces use.
    #[serde(default)]
    pub thread_id: Option<String>,
}

/// `GET /v1/workspace/file?path=[&thread_id=]` — a bounded, read-only preview
/// of one file under a workspace root (requirements/workspace-file-preview.md).
///
/// The authorization root follows the *same* resolution order as a turn: the
/// thread's own workspace override first, then the process lease. Using the
/// lease alone rejected a session's own files whenever that session was bound
/// to a project other than the process-wide active root.
///
/// The read is disk I/O with canonicalization, so it runs in `spawn_blocking`
/// (the `workspace/pick` precedent). The root is captured before the blocking
/// call: a concurrent switch changes only the *next* preview, matching the
/// snapshot semantics the renderer already applies to diff generations. No
/// usable root is a structured 404, not a guess.
pub async fn workspace_file(
    State(state): State<AppState>,
    Query(query): Query<WorkspaceFileQuery>,
) -> Result<Json<WorkspaceFilePreview>, ApiError> {
    let service = state
        .base
        .workspace
        .as_ref()
        .ok_or_else(|| missing("workspace"))?
        .clone();
    let lease = service
        .view()
        .active
        .map(|active| std::path::PathBuf::from(active.path))
        .ok_or_else(|| missing("active workspace"))?;
    let root = thread_workspace_root(&state, query.thread_id.as_deref())
        .await?
        .unwrap_or(lease);
    tokio::task::spawn_blocking(move || service.read_file(&root, &query.path))
        .await
        .map_err(|error| {
            ApiError::new(
                ark_common::ArkErrorCode::InternalError,
                format!("workspace file preview task failed: {error}"),
            )
        })?
        .map_err(ApiError)
        .map(Json)
}

/// Root a session's tool calls actually run in, when it overrides the lease.
///
/// The override is stored already normalized by the create/update boundary, so
/// this only reads it back; it never re-validates or re-normalizes the path.
/// A missing thread id stays an error, and a stored override is returned even
/// when its directory has since disappeared: falling back to the process root
/// would authorize a *different* project's file under the same name, which is
/// worse than letting `read_file` report that the root is unusable.
async fn thread_workspace_root(
    state: &AppState,
    raw: Option<&str>,
) -> Result<Option<std::path::PathBuf>, ApiError> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let thread = crate::api::handlers::threads::parse_thread(raw)?;
    let meta = state.store.get_thread(thread).await?;
    Ok(meta
        .runtime
        .workspace
        .as_deref()
        .map(str::trim)
        .filter(|workspace| !workspace.is_empty())
        .map(std::path::PathBuf::from))
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
    Ok(Json(service.upsert(entry).await?))
}

/// `DELETE /v1/mcp/servers/{id}`.
pub async fn mcp_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<McpServerListResponse>, ApiError> {
    let service = state.base.mcp.as_ref().ok_or_else(|| missing("mcp"))?;
    Ok(Json(service.delete(&id).await?))
}

/// `POST /v1/mcp/servers/{id}/test` — handshake plus `tools/list`.
pub async fn mcp_test(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<McpServerTestResponse>, ApiError> {
    let service = state.base.mcp.as_ref().ok_or_else(|| missing("mcp"))?;
    Ok(Json(service.test(&id).await))
}

// ---------------------------------------------------------------------------
// Plugins
// ---------------------------------------------------------------------------

/// `GET /v1/runtime/plugins`.
///
/// What the host actually serves, plus startup and egress diagnostics. The
/// report is read from the buses, so an unloaded plugin cannot leave a page or
/// tool behind here.
pub async fn plugins_report(
    State(state): State<AppState>,
) -> Result<Json<PluginHostReport>, ApiError> {
    let service = state
        .base
        .plugins
        .as_ref()
        .ok_or_else(|| missing("plugin"))?;
    Ok(Json(service.report()))
}

/// `GET /v1/runtime/plugins/{id}/provision` — enable-flow state of one plugin.
pub async fn plugin_provision(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PluginProvisionView>, ApiError> {
    let service = state
        .base
        .plugins
        .as_ref()
        .ok_or_else(|| missing("plugin"))?;
    Ok(Json(service.provision_status(&id)?))
}

/// `POST /v1/runtime/plugins/{id}/enable` — run the governed enable flow.
///
/// With `install: false` this only detects the declared tool and registers it
/// when present; a missing tool answers `needs_install` and the plugin stays
/// unusable. With `install: true` (the user's explicit consent) the declared
/// install command runs first; failure leaves the plugin `failed`/unusable.
pub async fn plugin_enable(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PluginEnableRequest>,
) -> Result<Json<PluginProvisionView>, ApiError> {
    let service = state
        .base
        .plugins
        .as_ref()
        .ok_or_else(|| missing("plugin"))?;
    Ok(Json(service.enable(&id, request).await?))
}

#[cfg(test)]
#[path = "base/tests.rs"]
mod tests;
