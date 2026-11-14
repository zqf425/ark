//! Workspace seeding and bulk-delete tests.
//!
//! Split out of `threads/tests.rs` (AGENTS §4 file-size limit); declared as a
//! descendant of that module via `#[path]`, so `use super::*` still reaches
//! the shared doubles (`state`, `test_dir`, `NoopRunner`, …) exactly as when
//! everything lived in one file.

use super::*;

/// `create_thread` must seed the configured default workspace so a new session
/// starts in the operator's chosen project instead of an unset composer pill.
#[tokio::test]
async fn a_new_thread_seeds_the_configured_default_workspace() {
    let dir = test_dir("seed-workspace");
    let sessions =
        Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).expect("session store"));
    let store: Arc<dyn SessionStore> = sessions;
    let config = RuntimeConfig {
        default_workspace: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    let state = AppState::new(
        store,
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(config)),
        Arc::new(UsageRecorder::new()),
        Arc::new(NoopRunner),
        dir.path().to_path_buf(),
        "test",
    )
    .with_base_services(BaseServices::new().with_providers(Arc::new(Providers)));

    let summary = crate::service::create_thread(&state, "seeded", None)
        .await
        .expect("create");
    let meta = state
        .store
        .get_thread(summary.thread_id)
        .await
        .expect("thread");
    assert_eq!(
        meta.runtime.workspace.as_deref(),
        Some(dir.path().to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn without_a_default_workspace_a_new_thread_stays_unset() {
    let dir = test_dir("no-default-workspace");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);

    let summary = crate::service::create_thread(&state, "plain", None)
        .await
        .expect("create");
    let meta = state
        .store
        .get_thread(summary.thread_id)
        .await
        .expect("thread");
    assert_eq!(meta.runtime.workspace, None);
}

// ---------------------------------------------------------------------------
// Per-project create + bulk delete (the rail's group header actions)
// ---------------------------------------------------------------------------

/// Workspace stub that accepts exactly the paths the tests use, mirroring the
/// real service's contract: an existing directory validates, a missing one is
/// a structured validation error. The active root is mutable so a test can
/// point the host's grouping fallback at a project and observe the sweep.
struct FixedWorkspace {
    active_root: std::sync::Mutex<Option<String>>,
}

impl FixedWorkspace {
    fn new(active_root: Option<&str>) -> Self {
        Self {
            active_root: std::sync::Mutex::new(active_root.map(str::to_string)),
        }
    }
}

#[async_trait::async_trait]
impl ark_ports::WorkspaceService for FixedWorkspace {
    fn validate(&self, raw: &str) -> ark_common::Result<std::path::PathBuf> {
        match raw {
            "/w/alpha" => Ok(std::path::PathBuf::from("/w/alpha")),
            "/w/beta" => Ok(std::path::PathBuf::from("/w/beta")),
            "missing" => Err(ark_common::ArkError::validation(
                "workspace path does not exist",
            )),
            other => Err(ark_common::ArkError::validation(format!(
                "unexpected workspace input: {other}"
            ))),
        }
    }

    fn status(&self, _path: &str) -> ark_contract::WorkspaceStatus {
        unreachable!("not used by this test")
    }

    fn read_file(
        &self,
        _root: &std::path::Path,
        _path: &str,
    ) -> ark_common::Result<ark_contract::WorkspaceFilePreview> {
        unreachable!("not used by this test")
    }

    fn view(&self) -> ark_contract::WorkspaceView {
        ark_contract::WorkspaceView {
            active: self
                .active_root
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .map(|path| ark_contract::WorkspaceDescriptor {
                    path,
                    generation: 1,
                }),
            recent: Vec::new(),
            generation: 1,
        }
    }

    fn switch(
        &self,
        _path: &str,
        _active_runs: usize,
    ) -> ark_common::Result<ark_contract::WorkspaceView> {
        unreachable!("not used by this test")
    }

    fn diff(&self) -> ark_common::Result<ark_contract::WorkspaceDiff> {
        unreachable!("not used by this test")
    }
}

fn workspace_state(data_dir: &std::path::Path) -> AppState {
    workspace_state_with_root(data_dir, None)
}

fn workspace_state_with_root(data_dir: &std::path::Path, active_root: Option<&str>) -> AppState {
    AppState::new(
        {
            let sessions = Arc::new(
                FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"),
            );
            let store: Arc<dyn SessionStore> = sessions;
            store
        },
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig::default())),
        Arc::new(UsageRecorder::new()),
        Arc::new(NoopRunner),
        data_dir.to_path_buf(),
        "test",
    )
    .with_base_services(
        BaseServices::new().with_workspace(Arc::new(FixedWorkspace::new(active_root))),
    )
}

/// An explicit `workspace` on `POST /v1/threads` seeds the override, so the
/// rail's per-project "new session" lands the thread in that project group.
#[tokio::test]
async fn create_with_workspace_seeds_the_thread_override() {
    let dir = test_dir("create-with-workspace");
    let state = workspace_state(dir.path());

    let summary = crate::service::create_thread(&state, "seeded", Some("/w/alpha"))
        .await
        .expect("create");
    let meta = state
        .store
        .get_thread(summary.thread_id)
        .await
        .expect("thread");
    assert_eq!(meta.runtime.workspace.as_deref(), Some("/w/alpha"));
    // The response must carry it too: the rail groups list rows by it.
    assert_eq!(summary.workspace.as_deref(), Some("/w/alpha"));
}

/// `DELETE /v1/threads?workspace=` removes every settled thread of the project
/// and keeps running ones, reporting each refusal structurally.
#[tokio::test]
async fn bulk_delete_removes_settled_threads_and_skips_running_ones() {
    let dir = test_dir("bulk-delete");
    let state = workspace_state(dir.path());

    let settled = crate::service::create_thread(&state, "settled", Some("/w/alpha"))
        .await
        .expect("settled")
        .thread_id;
    let running = crate::service::create_thread(&state, "running", Some("/w/alpha"))
        .await
        .expect("running")
        .thread_id;
    let other = crate::service::create_thread(&state, "other", Some("/w/beta"))
        .await
        .expect("other")
        .thread_id;
    let running_turn = TurnId::new();
    state
        .turns
        .try_register(running, running_turn, ark_agent::CancelToken::new())
        .expect("register running turn");

    let response = crate::service::delete_settled_by_workspace(&state, "/w/alpha")
        .await
        .expect("bulk delete");

    assert_eq!(response.deleted, vec![settled]);
    assert_eq!(response.skipped.len(), 1);
    assert_eq!(response.skipped[0].thread_id, running);
    assert_eq!(response.skipped[0].code, ark_common::ArkErrorCode::Conflict);
    assert!(
        state.store.get_thread(running).await.is_ok(),
        "a running thread survives the sweep"
    );
    assert!(
        state.store.get_thread(other).await.is_ok(),
        "another project's threads are untouched"
    );
    // The turn epoch marker of a surviving thread is reclaimed for none of its
    // business here, but the deleted thread must not leak one.
    assert_eq!(state.turns.latest_for(settled), None);
}

/// A thread whose newest durable turn-lifecycle event is still `turn_started`
/// counts as running even after a restart dropped the registry — the same
/// durable verdict the list's busy marker uses.
#[tokio::test]
async fn bulk_delete_respects_the_durable_running_verdict() {
    let dir = test_dir("bulk-delete-durable");
    let state = workspace_state(dir.path());

    let busy = crate::service::create_thread(&state, "busy", Some("/w/alpha"))
        .await
        .expect("busy")
        .thread_id;
    state
        .store
        .append_event(
            busy,
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 0,
                thread_id: busy,
                turn_id: TurnId::new(),
                timestamp: chrono::Utc::now(),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
        )
        .await
        .expect("seed turn_started");

    let response = crate::service::delete_settled_by_workspace(&state, "/w/alpha")
        .await
        .expect("bulk delete");

    assert!(response.deleted.is_empty());
    assert_eq!(response.skipped.len(), 1);
    assert_eq!(response.skipped[0].code, ark_common::ArkErrorCode::Conflict);
    assert!(state.store.get_thread(busy).await.is_ok());
}

/// The bulk verb refuses an absent workspace instead of silently sweeping the
/// default group.
#[tokio::test]
async fn bulk_delete_without_workspace_is_a_validation_error() {
    let dir = test_dir("bulk-delete-empty");
    let state = workspace_state(dir.path());
    let error = remove_by_workspace(
        State(state),
        Query(ThreadWorkspaceQuery { workspace: None }),
    )
    .await
    .expect_err("empty workspace is refused");
    assert_eq!(error.0.code, ark_common::ArkErrorCode::ValidationError);
}

/// The rail groups a thread without an override under the runtime's active
/// root, so the bulk verb must honor the same fallback or the UI's active
/// group would keep rows this delete already swept.
#[tokio::test]
async fn bulk_delete_matches_override_less_threads_against_the_active_root() {
    let dir = test_dir("bulk-delete-fallback");
    // The stub's active root is the grouping fallback for override-less rows,
    // mirroring the renderer's rail: no override means "the active root".
    let state = workspace_state_with_root(dir.path(), Some("/w/alpha"));

    let override_row = crate::service::create_thread(&state, "override", Some("/w/alpha"))
        .await
        .expect("override")
        .thread_id;
    let fallback_row = crate::service::create_thread(&state, "fallback", None)
        .await
        .expect("fallback")
        .thread_id;
    // An override pointing elsewhere must not match the fallback key.
    let elsewhere = crate::service::create_thread(&state, "elsewhere", Some("/w/beta"))
        .await
        .expect("elsewhere")
        .thread_id;

    let response = crate::service::delete_settled_by_workspace(&state, "/w/alpha")
        .await
        .expect("bulk delete");
    let mut deleted = response.deleted;
    deleted.sort();
    let mut expected = vec![fallback_row, override_row];
    expected.sort();
    assert_eq!(deleted, expected);
    assert!(
        state.store.get_thread(elsewhere).await.is_ok(),
        "another project's threads are untouched"
    );

    // With no active root the fallback key vanishes, so an override-less row
    // (the rail's default group) is never swept by a path-keyed delete.
    let dir2 = test_dir("bulk-delete-fallback-none");
    let state2 = workspace_state(dir2.path());
    let plain = crate::service::create_thread(&state2, "plain", None)
        .await
        .expect("plain")
        .thread_id;
    let response2 = crate::service::delete_settled_by_workspace(&state2, "/w/alpha")
        .await
        .expect("sweep");
    assert!(response2.deleted.is_empty());
    assert!(state2.store.get_thread(plain).await.is_ok());
}
