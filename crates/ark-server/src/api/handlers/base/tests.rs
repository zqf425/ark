//! Tests for the platform capability handlers.

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

    async fn destroy_child(&self, _parent: ThreadId, _child: ChildId) -> ark_common::Result<bool> {
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
