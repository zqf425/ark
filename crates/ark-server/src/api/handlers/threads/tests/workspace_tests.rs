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
    // The seed lands in the same normalized comparison form the explicit
    // create path uses (see `the_default_workspace_seed_is_normalized`).
    assert_eq!(
        meta.runtime.workspace.as_deref(),
        Some(ark_common::path::normalize(&dir.path().to_string_lossy()).as_str())
    );
}

/// default workspace 的种子与显式 `workspace` 参数走同一条归一化规则:
/// 配置里常是原生反斜杠形态(`~/.arkd/workspace`),不 normalize 就会让
/// 同一目录在 `thread.json` 里同时存在 `C:\w` 与 `C:/w` 两种键,侧栏分组
/// 按字符串精确比较时同一项目被拆成两组。
#[tokio::test]
async fn the_default_workspace_seed_is_normalized() {
    let dir = test_dir("seed-normalize");
    let sessions =
        Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).expect("session store"));
    let store: Arc<dyn SessionStore> = sessions;
    let config = RuntimeConfig {
        default_workspace: Some(dir.path().join("nested").join("project")),
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

    let expected =
        ark_common::path::normalize(&dir.path().join("nested/project").to_string_lossy());
    assert_eq!(
        summary.workspace.as_deref(),
        Some(expected.as_str()),
        "the default seed must land in the normalized comparison form"
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

/// 会话 pill 的切换走 `PATCH /v1/threads/{id}`(`merge_runtime`),前端发来的
/// 是 `GET /v1/workspace` 描述符里的 canonical 路径——Windows 上是反斜杠形态。
/// create 边界统一经 `ark_common::path::normalize` 规范化后落库,PATCH 边界
/// 必须同规则,否则同一目录会以两种字符串形态进入 `thread.json`,侧栏按
/// 原样字符串分组时把同一项目拆成两组(实测缺陷)。
#[tokio::test]
async fn patch_runtime_normalizes_the_workspace_override() {
    let dir = test_dir("patch-normalize");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    let thread = state.store.create_thread("switch").await.expect("thread");

    let patched = update(
        State(state.clone()),
        Path(thread.thread_id.to_string()),
        Json(UpdateThreadRequest {
            title: None,
            archived: None,
            runtime: Some(ark_contract::ThreadRuntimeSettings {
                workspace: Some("C:\\w\\alpha\\".to_string()),
                ..Default::default()
            }),
        }),
    )
    .await
    .expect("patch")
    .0;

    assert_eq!(
        patched.workspace.as_deref(),
        Some("C:/w/alpha"),
        "the stored override must be the normalized comparison key"
    );
}

/// PATCH 缺省 workspace 字段保持现状(契约注释明示),归一化不得把
/// "未提交" 误当成重置或改写存量值。
#[tokio::test]
async fn patch_without_a_workspace_keeps_the_stored_one_verbatim() {
    let dir = test_dir("patch-keep-ws");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    let thread = state.store.create_thread("keep").await.expect("thread");

    let first = update(
        State(state.clone()),
        Path(thread.thread_id.to_string()),
        Json(UpdateThreadRequest {
            title: None,
            archived: None,
            runtime: Some(ark_contract::ThreadRuntimeSettings {
                workspace: Some("/w/alpha".to_string()),
                ..Default::default()
            }),
        }),
    )
    .await
    .expect("first patch")
    .0;
    assert_eq!(first.workspace.as_deref(), Some("/w/alpha"));

    let second = update(
        State(state.clone()),
        Path(thread.thread_id.to_string()),
        Json(UpdateThreadRequest {
            title: Some("renamed".to_string()),
            archived: None,
            runtime: Some(ark_contract::ThreadRuntimeSettings {
                autonomy: Some("strict".to_string()),
                ..Default::default()
            }),
        }),
    )
    .await
    .expect("second patch")
    .0;

    assert_eq!(
        second.workspace.as_deref(),
        Some("/w/alpha"),
        "an absent workspace field must keep the stored value"
    );
}

/// 空白 workspace 覆盖在 PATCH 侧折叠为"未提交",与 create 边界对显式空串
/// 的处理一致,避免把无意义字符串种子成死覆盖。
#[tokio::test]
async fn patch_with_a_blank_workspace_stays_unset() {
    let dir = test_dir("patch-blank-ws");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    let thread = state.store.create_thread("blank").await.expect("thread");

    let patched = update(
        State(state.clone()),
        Path(thread.thread_id.to_string()),
        Json(UpdateThreadRequest {
            title: None,
            archived: None,
            runtime: Some(ark_contract::ThreadRuntimeSettings {
                workspace: Some("   ".to_string()),
                ..Default::default()
            }),
        }),
    )
    .await
    .expect("patch")
    .0;

    assert_eq!(
        patched.workspace, None,
        "a whitespace-only override must not be seeded"
    );
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
        path: &str,
        _active_runs: usize,
    ) -> ark_common::Result<ark_contract::WorkspaceView> {
        self.validate(path)?;
        *self
            .active_root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path.to_string());
        Ok(self.view())
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

#[tokio::test]
async fn bulk_delete_scan_does_not_block_unrelated_turn_registration() {
    let dir = test_dir("bulk-delete-scan-gate");
    let sessions =
        Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).expect("session store"));
    let settled = sessions.create_thread("settled").await.expect("thread");
    sessions
        .set_thread_runtime(
            settled.thread_id,
            ark_contract::ThreadRuntimeSettings {
                workspace: Some("/w/alpha".into()),
                ..Default::default()
            },
        )
        .await
        .expect("workspace");
    let unrelated = sessions.create_thread("unrelated").await.expect("thread");
    let gated = crate::api::handlers::test_support::GatedStore::new(
        sessions.clone() as Arc<dyn SessionStore>
    );
    gated.list.enable();
    let state = AppState::new(
        gated.clone(),
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig::default())),
        Arc::new(UsageRecorder::new()),
        Arc::new(NoopRunner),
        dir.path().to_path_buf(),
        "test",
    )
    .with_base_services(BaseServices::new().with_workspace(Arc::new(FixedWorkspace::new(None))));
    let entered = gated.list.entered.notified();
    let delete_task = tokio::spawn(remove_by_workspace(
        State(state.clone()),
        Query(ThreadWorkspaceQuery {
            workspace: Some("/w/alpha".into()),
        }),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(1), entered)
        .await
        .expect("bulk delete starts its scan");

    let registration = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        crate::service::start_turn(
            &state,
            unrelated.thread_id,
            "unrelated work".into(),
            Vec::new(),
        ),
    )
    .await;
    gated.list.resume();
    assert!(
        registration.is_ok(),
        "a bulk-delete scan must not hold the global turn-registration gate"
    );
    let response = tokio::time::timeout(std::time::Duration::from_secs(1), delete_task)
        .await
        .expect("bulk delete settles")
        .expect("delete task")
        .expect("bulk delete response")
        .0;
    assert_eq!(response.deleted, vec![settled.thread_id]);
    state.begin_shutdown();
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

#[tokio::test]
async fn bulk_delete_rechecks_the_active_workspace_before_deleting_fallback_rows() {
    let dir = test_dir("bulk-delete-fallback-race");
    let sessions =
        Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).expect("session store"));
    let fallback = sessions.create_thread("fallback").await.expect("thread");
    let gated = crate::api::handlers::test_support::GatedStore::new(
        sessions.clone() as Arc<dyn SessionStore>
    );
    gated.list.enable();
    let state = AppState::new(
        gated.clone(),
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig::default())),
        Arc::new(UsageRecorder::new()),
        Arc::new(NoopRunner),
        dir.path().to_path_buf(),
        "test",
    )
    .with_base_services(
        BaseServices::new().with_workspace(Arc::new(FixedWorkspace::new(Some("/w/alpha")))),
    );
    let entered = gated.list.entered.notified();
    let delete_state = state.clone();
    let delete_task = tokio::spawn(async move {
        crate::service::delete_settled_by_workspace(&delete_state, "/w/alpha").await
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), entered)
        .await
        .expect("bulk delete pauses after capturing the active root");

    state
        .base
        .workspace
        .as_ref()
        .expect("workspace service")
        .switch("/w/beta", 0)
        .expect("switch workspace");
    gated.list.resume();
    gated.list.disable();
    let response = tokio::time::timeout(std::time::Duration::from_secs(1), delete_task)
        .await
        .expect("bulk delete settles")
        .expect("delete task")
        .expect("bulk delete response");

    assert!(response.deleted.is_empty());
    assert_eq!(response.skipped.len(), 1);
    assert_eq!(response.skipped[0].code, ark_common::ArkErrorCode::Conflict);
    assert!(
        state.store.get_thread(fallback.thread_id).await.is_ok(),
        "a row that moved to another active workspace must survive the stale sweep"
    );
}

/// PATCH must join delete's workspace gate between effective-metadata
/// revalidation and the physical delete; otherwise a moved row is still swept.
#[tokio::test]
async fn bulk_delete_serializes_runtime_patch_with_its_delete_commit() {
    let dir = test_dir("bulk-delete-patch-race");
    let sessions =
        Arc::new(FileSessionStore::open(StoreConfig::new(dir.path())).expect("session store"));
    let thread = sessions.create_thread("settled").await.expect("thread");
    let gated = crate::api::handlers::test_support::GatedStore::new(
        sessions.clone() as Arc<dyn SessionStore>
    );
    let state = AppState::new(
        gated.clone(),
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig::default())),
        Arc::new(UsageRecorder::new()),
        Arc::new(NoopRunner),
        dir.path().to_path_buf(),
        "test",
    )
    .with_base_services(
        BaseServices::new()
            .with_workspace(Arc::new(FixedWorkspace::new(Some("/w/alpha"))))
            .with_providers(Arc::new(Providers)),
    );
    gated.delete.enable();
    let entered = gated.delete.entered.notified();
    let delete_state = state.clone();
    let delete_task = tokio::spawn(async move {
        crate::service::delete_settled_by_workspace(&delete_state, "/w/alpha").await
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), entered)
        .await
        .expect("delete reaches its gated commit");

    let patch_state = state.clone();
    let mut patch_task = tokio::spawn(async move {
        update(
            State(patch_state),
            Path(thread.thread_id.to_string()),
            Json(UpdateThreadRequest {
                runtime: Some(ark_contract::ThreadRuntimeSettings {
                    workspace: Some("/w/beta".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )
        .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut patch_task)
            .await
            .is_err(),
        "PATCH must wait for the delete commit gate"
    );
    gated.delete.resume();
    let response = delete_task
        .await
        .expect("delete task")
        .expect("bulk delete response");
    let _ = patch_task
        .await
        .expect("patch task")
        .expect_err("PATCH observes the committed deletion");

    assert_eq!(response.deleted, vec![thread.thread_id]);
    assert!(response.skipped.is_empty());
    assert!(matches!(
        state.store.get_thread(thread.thread_id).await,
        Err(error) if error.code == ark_common::ArkErrorCode::NotFound
    ));
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
