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
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use ark_agent::CancelToken;
    use ark_common::{ChildId, ThreadId, TurnId};
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{
        BaseServices, ChildRunRecord, SessionStore, SubagentRegistry, ToolRegistryPort,
        WorkspaceService,
    };
    use ark_store::{FileSessionStore, StoreConfig};
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use chrono::Utc;

    use super::*;

    struct EmptyTools;

    impl ToolRegistryPort for EmptyTools {
        fn names(&self) -> Vec<String> {
            Vec::new()
        }

        fn schemas(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }

        fn version(&self) -> u64 {
            0
        }
    }

    struct NoopRunner;

    #[async_trait::async_trait]
    impl crate::TurnRunner for NoopRunner {
        async fn run_turn(&self, _job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
            Ok(crate::TurnEnd::Completed)
        }
    }

    #[derive(Default)]
    struct Workspace {
        generation: AtomicU64,
        /// Root the last `read_file` call was authorized against.
        read_root: std::sync::Mutex<Option<PathBuf>>,
    }

    impl Workspace {
        /// Root the last `read_file` was authorized against, if any.
        fn read_root(&self) -> Option<PathBuf> {
            self.read_root
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl WorkspaceService for Workspace {
        fn validate(&self, raw: &str) -> ark_common::Result<PathBuf> {
            match raw {
                "../.." => Ok(PathBuf::from(r"C:\\workspace\\child\\..\\..")),
                "missing" => Err(ark_common::ArkError::validation(
                    "workspace path does not exist",
                )),
                _ => unreachable!("unexpected workspace input"),
            }
        }

        fn status(&self, _path: &str) -> ark_contract::WorkspaceStatus {
            unreachable!("not used by this test")
        }

        fn read_file(
            &self,
            root: &std::path::Path,
            path: &str,
        ) -> ark_common::Result<ark_contract::WorkspaceFilePreview> {
            // The preview tests assert *which* root the handler authorized, so
            // the stub records it instead of failing: the authorization
            // boundary is the behavior under test, not the file contents.
            *self
                .read_root
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(root.to_path_buf());
            Ok(ark_contract::WorkspaceFilePreview {
                path: path.to_string(),
                content: format!("read from {}", root.display()),
                truncated: false,
                binary: false,
                size_bytes: 0,
            })
        }

        fn view(&self) -> ark_contract::WorkspaceView {
            ark_contract::WorkspaceView {
                active: Some(ark_contract::WorkspaceDescriptor {
                    path: "/w/active".to_string(),
                    generation: self.generation.load(Ordering::Relaxed),
                }),
                recent: Vec::new(),
                generation: self.generation.load(Ordering::Relaxed),
            }
        }

        fn switch(
            &self,
            _path: &str,
            active_runs: usize,
        ) -> ark_common::Result<ark_contract::WorkspaceView> {
            if active_runs != 0 {
                return Err(ark_common::ArkError::conflict(format!(
                    "cannot switch workspace while {active_runs} turn(s) are active"
                )));
            }
            Ok(ark_contract::WorkspaceView {
                active: None,
                recent: Vec::new(),
                generation: self.generation.fetch_add(1, Ordering::Relaxed) + 1,
            })
        }

        fn diff(&self) -> ark_common::Result<ark_contract::WorkspaceDiff> {
            unreachable!("not used by this test")
        }
    }

    struct ActiveChildRegistry;

    #[async_trait::async_trait]
    impl SubagentRegistry for ActiveChildRegistry {
        fn list(&self, _parent: Option<ThreadId>) -> Vec<ChildRunRecord> {
            vec![ChildRunRecord::queued(
                ChildId::new(),
                ThreadId::new(),
                TurnId::new(),
                "active child",
                Utc::now(),
            )]
        }

        fn get(&self, _child: ChildId) -> Option<ChildRunRecord> {
            None
        }

        fn abort(&self, _child: ChildId) -> bool {
            false
        }

        fn steer(&self, _child: ChildId, _text: &str) -> bool {
            false
        }

        async fn destroy_child(
            &self,
            _parent: ThreadId,
            _child: ChildId,
        ) -> ark_common::Result<bool> {
            Ok(false)
        }

        fn active_count(&self) -> u32 {
            1
        }

        fn is_enabled(&self) -> bool {
            true
        }
    }

    /// A fresh random data directory, removed when the guard drops.
    ///
    /// `tempfile` names it uniquely, so parallel tests never share a store
    /// directory (and never contend for the same data lock).
    fn test_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("ark-workspace-")
            .tempdir()
            .expect("temp dir is creatable")
    }

    /// `BaseServices` stores the workspace port behind a trait object, so tests
    /// need the unsizing step spelled out.
    fn workspace_port(workspace: &Arc<Workspace>) -> Arc<dyn WorkspaceService> {
        workspace.clone()
    }

    fn state(data_dir: &Path) -> AppState {
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"));
        let store: Arc<dyn SessionStore> = sessions;
        AppState::new(
            store,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(NoopRunner),
            data_dir.to_path_buf(),
            "test",
        )
        .with_base_services(BaseServices::new().with_workspace(Arc::new(Workspace::default())))
    }

    async fn assert_workspace_switch_conflict(state: AppState, expected_active: usize) {
        let error = workspace_switch(
            State(state),
            Json(SwitchWorkspaceRequest {
                path: "other".to_string(),
            }),
        )
        .await
        .expect_err("active work must block switching");

        assert_eq!(error.0.code, ark_common::ArkErrorCode::Conflict);
        assert!(error.0.message.contains(&expected_active.to_string()));
        assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn active_parent_turn_makes_workspace_switch_return_409() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let state = state(&data_dir);
        state
            .turns
            .try_register(ThreadId::new(), TurnId::new(), CancelToken::new())
            .expect("register active parent");

        assert_workspace_switch_conflict(state, 1).await;
    }

    #[tokio::test]
    async fn active_child_run_makes_workspace_switch_return_409() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let mut state = state(&data_dir);
        state.subagents.registry = Some(Arc::new(ActiveChildRegistry));

        assert_workspace_switch_conflict(state, 1).await;
    }

    #[tokio::test]
    async fn idle_workspace_switch_returns_incremented_generation() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let state = state(&data_dir);

        for expected in [1, 2] {
            let response = workspace_switch(
                State(state.clone()),
                Json(SwitchWorkspaceRequest {
                    path: "other".to_string(),
                }),
            )
            .await
            .expect("idle switch");
            assert_eq!(response.0.generation, expected);
        }
    }

    #[tokio::test]
    async fn workspace_view_lists_thread_bound_projects_without_a_global_switch() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let state = state(&data_dir);
        // A session whose workspace was set only at the thread level (the
        // composer pick writes exactly this) must surface in `GET /v1/workspace`
        // even though the global switch history never saw the path.
        let thread = state.store.create_thread("ark").await.expect("thread");
        let mut runtime = thread.runtime.clone();
        runtime.workspace = Some("/code/ark".to_string());
        state
            .store
            .set_thread_runtime(thread.thread_id, runtime)
            .await
            .expect("bind workspace");

        let view = workspace_view(State(state)).await.expect("view");
        let paths: Vec<&str> = view.recent.iter().map(|d| d.path.as_str()).collect();
        assert!(
            paths.contains(&"/code/ark"),
            "thread-bound workspace must be offered in the project menu: {paths:?}"
        );
    }

    #[tokio::test]
    async fn workspace_view_orders_thread_projects_by_recency() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let state = state(&data_dir);
        for (title, workspace) in [("old", "/w/old"), ("new", "/w/new")] {
            let thread = state.store.create_thread(title).await.expect("thread");
            let mut runtime = thread.runtime.clone();
            runtime.workspace = Some(workspace.to_string());
            state
                .store
                .set_thread_runtime(thread.thread_id, runtime)
                .await
                .expect("bind workspace");
        }
        // Make "new" the most recently updated thread; the merged list must
        // surface its project first so the menu mirrors the rail's ordering.
        state
            .store
            .rename_thread(
                state
                    .store
                    .list_threads()
                    .await
                    .expect("rows")
                    .iter()
                    .find(|meta| meta.title == "new")
                    .expect("new row")
                    .thread_id,
                "new-renamed",
            )
            .await
            .expect("touch new thread");

        let view = workspace_view(State(state)).await.expect("view");
        let paths: Vec<&str> = view.recent.iter().map(|d| d.path.as_str()).collect();
        let new_index = paths
            .iter()
            .position(|p| *p == "/w/new")
            .expect("new project");
        let old_index = paths
            .iter()
            .position(|p| *p == "/w/old")
            .expect("old project");
        assert!(
            new_index < old_index,
            "most recently active project first: {paths:?}"
        );
    }

    /// The regression this route exists to prevent: a session bound to another
    /// project must be previewed against *its* root, not the process lease.
    #[tokio::test]
    async fn file_preview_authorizes_against_the_thread_workspace() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let mut state = state(&data_dir);
        let workspace = Arc::new(Workspace::default());
        state.base.workspace = Some(workspace_port(&workspace));
        let thread = state.store.create_thread("bound").await.expect("thread");
        let mut runtime = thread.runtime.clone();
        runtime.workspace = Some("/w/session".to_string());
        state
            .store
            .set_thread_runtime(thread.thread_id, runtime)
            .await
            .expect("bind workspace");

        let preview = workspace_file(
            State(state),
            Query(WorkspaceFileQuery {
                path: "docs/a.md".to_string(),
                thread_id: Some(thread.thread_id.to_string()),
            }),
        )
        .await
        .expect("preview");

        assert_eq!(preview.0.path, "docs/a.md");
        assert_eq!(
            workspace.read_root(),
            Some(PathBuf::from("/w/session")),
            "the session override must authorize the read"
        );
    }

    /// Without `thread_id` the process lease stays authoritative: the settings
    /// and review surfaces read files that belong to no session.
    #[tokio::test]
    async fn file_preview_without_a_thread_uses_the_active_root() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let mut state = state(&data_dir);
        let workspace = Arc::new(Workspace::default());
        state.base.workspace = Some(workspace_port(&workspace));

        let preview = workspace_file(
            State(state),
            Query(WorkspaceFileQuery {
                path: "a.md".to_string(),
                thread_id: None,
            }),
        )
        .await
        .expect("preview");

        assert_eq!(preview.0.content, "read from /w/active");
        assert_eq!(workspace.read_root(), Some(PathBuf::from("/w/active")));
    }

    /// A thread that does not exist must not fall back to the lease: that would
    /// answer a request for a deleted session with another project's files.
    #[tokio::test]
    async fn file_preview_for_an_unknown_thread_is_not_found() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let error = workspace_file(
            State(state(&data_dir)),
            Query(WorkspaceFileQuery {
                path: "a.md".to_string(),
                thread_id: Some(ThreadId::new().to_string()),
            }),
        )
        .await
        .expect_err("an unknown thread must not be previewed");

        assert_eq!(error.0.code, ark_common::ArkErrorCode::NotFound);
    }

    /// A session without its own override inherits the lease, exactly like a
    /// turn does; the handler must not invent a root of its own.
    #[tokio::test]
    async fn file_preview_for_an_unbound_thread_uses_the_active_root() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let mut state = state(&data_dir);
        let workspace = Arc::new(Workspace::default());
        state.base.workspace = Some(workspace_port(&workspace));
        let thread = state.store.create_thread("unbound").await.expect("thread");

        let preview = workspace_file(
            State(state),
            Query(WorkspaceFileQuery {
                path: "a.md".to_string(),
                thread_id: Some(thread.thread_id.to_string()),
            }),
        )
        .await
        .expect("preview");

        assert_eq!(preview.0.content, "read from /w/active");
        assert_eq!(workspace.read_root(), Some(PathBuf::from("/w/active")));
    }

    #[tokio::test]
    async fn workspace_validation_returns_a_normalized_path() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let response = workspace_validate(
            State(state(&data_dir)),
            Json(ValidateWorkspaceBody {
                path: "../..".to_string(),
            }),
        )
        .await
        .expect("validated workspace");

        assert!(response.0.ok);
        assert_eq!(response.0.path, "C:/");
    }

    #[tokio::test]
    async fn workspace_validation_rejects_a_missing_path() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let error = workspace_validate(
            State(state(&data_dir)),
            Json(ValidateWorkspaceBody {
                path: "missing".to_string(),
            }),
        )
        .await
        .expect_err("missing workspace must be rejected");

        assert_eq!(error.0.code, ark_common::ArkErrorCode::ValidationError);
        assert!(error.0.message.contains("does not exist"));
    }

    #[tokio::test]
    async fn folder_pick_without_the_port_is_runtime_unavailable() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let error = workspace_pick(State(state(&data_dir)))
            .await
            .expect_err("no folder dialog service is wired in this state");

        assert_eq!(error.0.code, ark_common::ArkErrorCode::RuntimeUnavailable);
        assert!(error.0.message.contains("folder dialog"));
    }
}
