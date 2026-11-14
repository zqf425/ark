use std::sync::Arc;

use super::*;
use ark_common::{ChildId, TurnId};
use ark_config::{ConfigStore, RuntimeConfig};
use ark_obs::UsageRecorder;
use ark_ports::{
    BaseServices, ModelEndpoint, ProviderService, SessionStore, ThreadMeta, ToolRegistryPort,
};
use ark_store::{FileSessionStore, StoreConfig};
use axum::extract::{Path, State};

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

struct Providers;

#[async_trait::async_trait]
impl ProviderService for Providers {
    fn list(&self) -> ark_contract::ProviderListResponse {
        ark_contract::ProviderListResponse::default()
    }

    fn get(&self, _id: &str) -> ark_common::Result<ark_contract::ProviderEntry> {
        unreachable!("not used by this test")
    }

    fn upsert(
        &self,
        _entry: ark_contract::ProviderEntry,
        _make_default: bool,
    ) -> ark_common::Result<ark_contract::ProviderListResponse> {
        unreachable!("not used by this test")
    }

    fn delete(&self, _id: &str) -> ark_common::Result<ark_contract::ProviderListResponse> {
        unreachable!("not used by this test")
    }

    fn set_default(
        &self,
        _id: &str,
        _model: Option<&str>,
    ) -> ark_common::Result<ark_contract::ProviderListResponse> {
        unreachable!("not used by this test")
    }

    async fn test(
        &self,
        _request: ark_contract::ProviderTestRequest,
    ) -> ark_contract::ProviderTestResponse {
        unreachable!("not used by this test")
    }

    async fn models(
        &self,
        _request: ark_contract::ProviderModelCatalogRequest,
    ) -> ark_common::Result<ark_contract::ProviderModelCatalogResponse> {
        unreachable!("not used by this test")
    }

    fn resolve_endpoint(&self, id: Option<&str>) -> Option<ModelEndpoint> {
        (id == Some("known")).then(|| ModelEndpoint {
            provider_id: "known".to_string(),
            model: "model".to_string(),
            base_url: "https://example.invalid".to_string(),
            api_key: "secret".to_string(),
            endpoint_format: "openai".to_string(),
            custom_endpoint: None,
            max_tokens: None,
            temperature: None,
            reasoning_effort: None,
        })
    }
}

fn state(data_dir: &std::path::Path) -> AppState {
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
    .with_base_services(BaseServices::new().with_providers(Arc::new(Providers)))
}

/// A fresh random data directory, removed when the guard drops.
///
/// `tempfile` names it uniquely, so two tests running in parallel can never
/// share one store directory or its lock file.
fn test_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("ark-thread-{tag}-"))
        .tempdir()
        .expect("temp dir is creatable")
}

#[test]
fn invalid_thread_ids_are_validation_errors() {
    let error = parse_thread("not-a-uuid").unwrap_err();
    assert_eq!(error.0.code, ark_common::ArkErrorCode::ValidationError);
}

#[test]
fn valid_thread_ids_roundtrip() {
    let id = ThreadId::new();
    assert_eq!(parse_thread(&id.to_string()).unwrap(), id);
}

#[test]
fn summary_mapping_preserves_title_and_archive_flag() {
    let meta = ThreadMeta {
        thread_id: ThreadId::new(),
        title: "hello".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        archived: true,
        runtime: ark_contract::ThreadRuntimeSettings::default(),
        forked_from: None,
        fork_at_seq: None,
        inherited_item_count: None,
        last_seq: None,
        last_event_at: None,
    };
    let summary = summary_of(&meta);
    assert_eq!(summary.title, "hello");
    assert!(summary.archived);
    assert_eq!(summary.last_seq, None);
    // Metadata alone never asserts liveness; only the list/detail handlers can.
    assert!(!summary.running);
}

#[test]
fn summary_mapping_carries_watermark_and_fork_provenance() {
    let source = ThreadId::new();
    let meta = ThreadMeta {
        thread_id: ThreadId::new(),
        title: "fork".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        archived: false,
        runtime: ark_contract::ThreadRuntimeSettings {
            workspace: Some("/tmp/project".into()),
            ..Default::default()
        },
        forked_from: Some(source),
        fork_at_seq: Some(12),
        inherited_item_count: Some(5),
        last_seq: Some(17),
        last_event_at: Some(Utc::now()),
    };
    let summary = summary_of(&meta);
    assert_eq!(summary.last_seq, Some(17));
    assert_eq!(summary.forked_from, Some(source));
    assert_eq!(summary.fork_at_seq, Some(12));
    assert_eq!(summary.inherited_item_count, Some(5));
    assert_eq!(summary.workspace.as_deref(), Some("/tmp/project"));
    assert!(summary.last_event_at.is_some());
}

#[test]
fn thread_detail_keeps_only_the_latest_distinct_completion_cards() {
    let thread = ThreadId::new();
    let events = (1..=55)
        .map(|seq| RuntimeEvent::ChildCompleted {
            seq,
            thread_id: thread,
            timestamp: Utc::now(),
            notification_id: format!("child:{seq}"),
            child_id: ChildId::new(),
            label: "inspect runtime".to_string(),
            status: ark_common::ChildRunStatus::Completed,
            summary: None,
            error: None,
            usage: None,
            tool_invocations: None,
        })
        .collect();

    let notices = recent_child_completions(events);

    assert_eq!(notices.len(), MAX_CHILD_COMPLETION_NOTICES);
    assert_eq!(notices.first().unwrap().seq, 6);
    assert_eq!(notices.last().unwrap().seq, 55);
}

#[test]
fn interaction_projection_keeps_only_unresolved_live_gate_for_active_turn() {
    let thread = ThreadId::new();
    let active = TurnId::new();
    let finished = TurnId::new();
    let now = Utc::now();
    let runtime = project_runtime_snapshot(
        Some(ark_contract::TurnSummary {
            turn_id: active,
            thread_id: thread,
            state: "running".into(),
            started_at: now,
            ended_at: None,
            error: None,
        }),
        vec![
            RuntimeEvent::ApprovalRequested {
                seq: 1,
                thread_id: thread,
                turn_id: active,
                timestamp: now,
                approval_id: "live".into(),
                tool: "bash".into(),
                summary: "run command".into(),
            },
            RuntimeEvent::ApprovalRequested {
                seq: 2,
                thread_id: thread,
                turn_id: active,
                timestamp: now,
                approval_id: "resolved".into(),
                tool: "write".into(),
                summary: "update file".into(),
            },
            RuntimeEvent::ApprovalResolved {
                seq: 3,
                thread_id: thread,
                turn_id: active,
                timestamp: now,
                approval_id: "resolved".into(),
                approved: true,
            },
            RuntimeEvent::UserInputRequested {
                seq: 4,
                thread_id: thread,
                turn_id: finished,
                timestamp: now,
                request_id: "old".into(),
                prompt: "old prompt".into(),
            },
        ],
        ["live"],
        std::iter::empty::<&str>(),
    );

    assert_eq!(runtime.active_turn.unwrap().turn_id, active);
    assert_eq!(runtime.pending_approvals.len(), 1);
    assert_eq!(runtime.pending_approvals[0].approval_id, "live");
    assert!(runtime.pending_user_inputs.is_empty());
}

/// Seed threads with distinct titles and archive flags, oldest update first.
async fn seed_threads(state: &AppState, count: usize) -> Vec<ThreadMeta> {
    let mut metas = Vec::new();
    for i in 0..count {
        let mut meta = state
            .store
            .create_thread(&format!("thread-{i:02}"))
            .await
            .expect("thread");
        if i % 2 == 1 {
            meta = state
                .store
                .set_archived(meta.thread_id, true)
                .await
                .expect("archive");
        }
        metas.push(meta);
    }
    metas
}

async fn list_response(state: AppState, query: ThreadListQuery) -> Vec<ThreadSummary> {
    list(State(state), Query(query)).await.expect("list").0
}

#[tokio::test]
async fn list_without_query_stays_compatible() {
    let dir = test_dir("compat");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    // More than THREAD_PAGE_DEFAULT_LIMIT: an absent `limit` must keep listing
    // every thread, not silently truncate to the page default.
    seed_threads(&state, 55).await;

    let summaries = list_response(state.clone(), ThreadListQuery::default()).await;

    assert_eq!(summaries.len(), 55);
    // Deterministic order: active first, then most recently updated, so
    // the newest active thread (seeded last, even index) leads.
    assert_eq!(summaries[0].title, "thread-54");
}

#[tokio::test]
async fn list_marks_only_threads_with_an_in_flight_turn_as_running() {
    let dir = test_dir("running");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    state.store.create_thread("idle").await.expect("thread");
    let busy = state
        .store
        .create_thread("busy")
        .await
        .expect("thread")
        .thread_id;
    let busy_turn = TurnId::new();
    state
        .turns
        .try_register(busy, busy_turn, ark_agent::CancelToken::new())
        .expect("register busy");

    let summaries = list_response(state.clone(), ThreadListQuery::default()).await;
    let running: Vec<_> = summaries
        .iter()
        .filter(|summary| summary.running)
        .map(|summary| summary.thread_id)
        .collect();
    assert_eq!(running, vec![busy], "only the registered thread is busy");
}

/// A thread whose newest durable turn event is `turn_started` (no terminal
/// record yet) reads busy even when the registry forgot it — a restart drops
/// the process-local registry, and the durable log is the only surviving
/// evidence of the in-flight turn (smart-ark busy rule).
#[tokio::test]
async fn list_falls_back_to_the_durable_log_for_running_after_a_registry_loss() {
    let dir = test_dir("running-durable");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    let open = state
        .store
        .create_thread("open-turn")
        .await
        .expect("thread")
        .thread_id;
    let settled = state
        .store
        .create_thread("settled-turn")
        .await
        .expect("thread")
        .thread_id;
    let idle = state
        .store
        .create_thread("idle")
        .await
        .expect("thread")
        .thread_id;
    let turn = TurnId::new();
    state
        .store
        .append_event(
            open,
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 0,
                thread_id: open,
                turn_id: turn,
                timestamp: Utc::now(),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
        )
        .await
        .expect("open turn");
    let done = TurnId::new();
    state
        .store
        .append_event(
            settled,
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 0,
                thread_id: settled,
                turn_id: done,
                timestamp: Utc::now(),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
        )
        .await
        .expect("settled turn starts");
    state
        .store
        .append_event(
            settled,
            RuntimeEvent::TurnCompleted {
                seq: 0,
                thread_id: settled,
                turn_id: done,
                timestamp: Utc::now(),
            },
        )
        .await
        .expect("settled turn completes");
    // No registry entry anywhere: the simulation of a restart.

    let summaries = list_response(state.clone(), ThreadListQuery::default()).await;
    let running: Vec<_> = summaries
        .iter()
        .filter(|summary| summary.running)
        .map(|summary| summary.thread_id)
        .collect();
    assert_eq!(
        running,
        vec![open],
        "an unterminated durable turn reads busy; a settled one does not"
    );
    assert!(
        !running.contains(&idle),
        "a thread with no turn events is idle"
    );
}

#[tokio::test]
async fn list_orders_by_last_event_time_not_metadata_updates() {
    let dir = test_dir("activity-order");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    // `older` is created (and its metadata touched) after `newer`, but only
    // `newer` receives events: the activity order must still lead with it,
    // which is the "the thread that just streamed first" behaviour the rail
    // promises. A pure `updated_at` order would put `older` on top.
    let older = state
        .store
        .create_thread("older")
        .await
        .expect("thread")
        .thread_id;
    std::thread::sleep(std::time::Duration::from_millis(5));
    let newer = state
        .store
        .create_thread("newer")
        .await
        .expect("thread")
        .thread_id;
    let turn = TurnId::new();
    state
        .store
        .append_event(
            older,
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 0,
                thread_id: older,
                turn_id: turn,
                timestamp: Utc::now() - chrono::Duration::seconds(30),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
        )
        .await
        .expect("older event");
    state
        .store
        .append_event(
            newer,
            RuntimeEvent::TurnCompleted {
                seq: 0,
                thread_id: newer,
                turn_id: turn,
                timestamp: Utc::now(),
            },
        )
        .await
        .expect("newer event");

    let summaries = list_response(state.clone(), ThreadListQuery::default()).await;
    let titles: Vec<&str> = summaries.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["newer", "older"]);
    // The activity projection must survive into the wire row.
    let newer_row = summaries.iter().find(|s| s.title == "newer").unwrap();
    assert!(newer_row.last_event_at.is_some());
}

#[tokio::test]
async fn limit_one_returns_exactly_one_page() {
    let dir = test_dir("limit");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    seed_threads(&state, 4).await;

    let page = list_response(
        state.clone(),
        ThreadListQuery {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(page.len(), 1);
    assert_eq!(page[0].title, "thread-02");
}

#[tokio::test]
async fn offset_walks_pages_without_repetition_or_gap() {
    let dir = test_dir("offset");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    seed_threads(&state, 5).await;

    let first = list_response(
        state.clone(),
        ThreadListQuery {
            limit: Some(2),
            ..Default::default()
        },
    )
    .await;
    let second = list_response(
        state.clone(),
        ThreadListQuery {
            limit: Some(2),
            offset: Some(2),
            ..Default::default()
        },
    )
    .await;
    let third = list_response(
        state.clone(),
        ThreadListQuery {
            limit: Some(2),
            offset: Some(4),
            ..Default::default()
        },
    )
    .await;

    let titles: Vec<&str> = first
        .iter()
        .chain(&second)
        .chain(&third)
        .map(|s| s.title.as_str())
        .collect();
    assert_eq!(
        titles,
        vec![
            "thread-04",
            "thread-02",
            "thread-00",
            "thread-03",
            "thread-01"
        ]
    );
}

#[tokio::test]
async fn limit_clamps_zero_and_oversized_values() {
    let dir = test_dir("clamp");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    seed_threads(&state, 3).await;

    // limit=0 clamps to 1: still exactly one row, never an empty page.
    let zero = list_response(
        state.clone(),
        ThreadListQuery {
            limit: Some(0),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(zero.len(), 1);
    // Oversized values clamp to the contract maximum.
    let huge = list_response(
        state.clone(),
        ThreadListQuery {
            limit: Some(u32::MAX),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(huge.len(), 3);
}

#[tokio::test]
async fn archived_filter_composes_with_pagination() {
    let dir = test_dir("archived");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    seed_threads(&state, 4).await;

    let archived = list_response(
        state.clone(),
        ThreadListQuery {
            archived: Some(true),
            ..Default::default()
        },
    )
    .await;
    assert!(archived.iter().all(|s| s.archived));
    assert_eq!(archived.len(), 2);

    let active_paged = list_response(
        state.clone(),
        ThreadListQuery {
            archived: Some(false),
            limit: Some(1),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(active_paged.len(), 1);
    assert!(!active_paged[0].archived);
}

#[tokio::test]
async fn title_search_composes_with_pagination() {
    let dir = test_dir("search");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    for title in ["rust review", "rust design", "other"] {
        state.store.create_thread(title).await.expect("thread");
    }

    let hits = list_response(
        state.clone(),
        ThreadListQuery {
            q: Some("RUST".into()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.starts_with("rust"));
}

#[tokio::test]
async fn unknown_provider_patch_is_rejected_without_changing_runtime() {
    let dir = test_dir("provider");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    let thread = state.store.create_thread("provider").await.expect("thread");

    let error = update(
        State(state.clone()),
        Path(thread.thread_id.to_string()),
        Json(UpdateThreadRequest {
            title: None,
            archived: None,
            runtime: Some(ark_contract::ThreadRuntimeSettings {
                provider_id: Some("does-not-exist".to_string()),
                ..Default::default()
            }),
        }),
    )
    .await
    .expect_err("unknown provider must be rejected");

    assert_eq!(error.0.code, ark_common::ArkErrorCode::ValidationError);
    assert_eq!(error.0.code.http_status(), 400);
    assert_eq!(
        state
            .store
            .get_thread(thread.thread_id)
            .await
            .expect("thread")
            .runtime
            .provider_id,
        None
    );
}

/// PATCH 写入的会话级覆盖必须能被 GET 详情原样读回，否则客户端只能去读
/// `thread.json` 才能确认当前覆盖（验收 D3）。
#[tokio::test]
async fn patch_runtime_overrides_are_echoed_by_the_detail_response() {
    let dir = test_dir("echo");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    let thread = state.store.create_thread("echo").await.expect("thread");

    let patched = update(
        State(state.clone()),
        Path(thread.thread_id.to_string()),
        Json(UpdateThreadRequest {
            title: None,
            archived: None,
            runtime: Some(ark_contract::ThreadRuntimeSettings {
                workspace: Some("/tmp/echo-workspace".to_string()),
                autonomy: Some("unattended".to_string()),
                ..Default::default()
            }),
        }),
    )
    .await
    .expect("patch")
    .0;
    assert_eq!(patched.title, "echo");

    let detail = get(State(state.clone()), Path(thread.thread_id.to_string()))
        .await
        .expect("detail")
        .0;

    assert_eq!(detail.runtime.autonomy.as_deref(), Some("unattended"));
    assert_eq!(
        detail.runtime.workspace.as_deref(),
        Some("/tmp/echo-workspace")
    );
}

/// 没有覆盖的线程必须回显空覆盖，而不是把全局默认值伪装成会话级设置。
#[tokio::test]
async fn detail_reports_no_overrides_when_the_thread_has_none() {
    let dir = test_dir("noecho");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    let thread = state.store.create_thread("plain").await.expect("thread");

    let detail = get(State(state.clone()), Path(thread.thread_id.to_string()))
        .await
        .expect("detail")
        .0;

    assert_eq!(detail.runtime.autonomy, None);
    assert_eq!(detail.runtime.workspace, None);
    assert_eq!(detail.runtime.provider_id, None);
    assert_eq!(detail.runtime.model, None);
}

/// PATCH 未提交的字段保留现状：语义在契约注释中明示，这里锁住行为。
#[tokio::test]
async fn patch_without_a_field_keeps_the_existing_override() {
    let dir = test_dir("keep");
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
                autonomy: Some("strict".to_string()),
                workspace: Some("/tmp/keep-me".to_string()),
                ..Default::default()
            }),
        }),
    )
    .await
    .expect("first patch")
    .0;
    assert_eq!(first.title, "keep");

    let second = update(
        State(state.clone()),
        Path(thread.thread_id.to_string()),
        Json(UpdateThreadRequest {
            title: Some("renamed".to_string()),
            archived: None,
            runtime: None,
        }),
    )
    .await
    .expect("second patch")
    .0;
    assert_eq!(second.title, "renamed");

    let detail = get(State(state.clone()), Path(thread.thread_id.to_string()))
        .await
        .expect("detail")
        .0;

    assert_eq!(detail.runtime.autonomy.as_deref(), Some("strict"));
    assert_eq!(detail.runtime.workspace.as_deref(), Some("/tmp/keep-me"));
    assert_eq!(detail.summary.title, "renamed");
}

/// 线级回归：真实路由 + 序列化之后，PATCH 写入的覆盖仍然出现在 GET 响应里。
///
/// 处理器级断言只能证明 DTO 字段被填上，这里再证明 HTTP 边界上确实可见，
/// 这正是验收 D3 关心的"从 API 就能确认当前覆盖"。
#[tokio::test]
async fn the_wire_detail_exposes_patched_runtime_overrides() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    let dir = test_dir("wire");
    let data_dir = dir.path().to_path_buf();
    let app = crate::api::routes::router(state(&data_dir));

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/threads")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":"wire"}"#))
                .unwrap(),
        )
        .await
        .expect("create");
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .expect("create body");
    let thread_id = serde_json::from_slice::<serde_json::Value>(&body).expect("create json")
        ["thread_id"]
        .as_str()
        .expect("thread id")
        .to_string();

    let patched = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/v1/threads/{thread_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"runtime":{"autonomy":"strict","workspace":"/tmp/wire-ws"}}"#,
                ))
                .unwrap(),
        )
        .await
        .expect("patch");
    assert_eq!(patched.status(), StatusCode::OK);

    let detail = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/threads/{thread_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("detail");
    assert_eq!(detail.status(), StatusCode::OK);
    let body = axum::body::to_bytes(detail.into_body(), usize::MAX)
        .await
        .expect("detail body");
    let detail: serde_json::Value = serde_json::from_slice(&body).expect("detail json");

    assert_eq!(detail["runtime"]["autonomy"], "strict");
    assert_eq!(detail["runtime"]["workspace"], "/tmp/wire-ws");
}

/// 回归：一个子代理只应产生一个 `child_ids` 条目。
///
/// `children/runs.jsonl` 是 append-only，`queued`/`running`/`completed` 三行都
/// 属于同一个 child；`load_detail` 曾经把原始日志逐行映射成 id，于是同一
/// child 在 `child_ids` 里出现三次。这里走真实 store 的完整写入路径。
#[tokio::test]
async fn detail_lists_one_child_id_per_delegated_child() {
    let dir = test_dir("child-ids");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);
    let parent = state.store.create_thread("parent").await.expect("thread");
    let child = ChildId::new();
    state
        .store
        .create_child_run(parent.thread_id, child, "scan")
        .await
        .expect("child dir");
    for status in [
        ark_common::ChildRunStatus::Queued,
        ark_common::ChildRunStatus::Running,
        ark_common::ChildRunStatus::Completed,
    ] {
        let mut record = ark_ports::ChildRunRecord::queued(
            child,
            parent.thread_id,
            TurnId::new(),
            "scan",
            Utc::now(),
        );
        record.status = status;
        if status == ark_common::ChildRunStatus::Completed {
            record.ended_at = Some(Utc::now());
        }
        state.store.append_child_run(record).await.expect("append");
    }

    let records = state.store.list_child_runs(parent.thread_id).await.unwrap();
    assert_eq!(
        records.len(),
        3,
        "the log stays append-only: one row per status change"
    );

    let detail = load_detail(&state, parent.thread_id).await.expect("detail");
    assert_eq!(
        detail.child_ids,
        vec![child],
        "the projection must fold the log to one id per child"
    );
}

#[tokio::test]
async fn removing_a_thread_forgets_its_turn_epoch_but_keeps_other_threads() {
    let dir = test_dir("forget-epoch");
    let data_dir = dir.path().to_path_buf();
    let state = state(&data_dir);

    let doomed = state
        .store
        .create_thread("doomed")
        .await
        .expect("thread")
        .thread_id;
    let survivor = state
        .store
        .create_thread("survivor")
        .await
        .expect("thread")
        .thread_id;
    let doomed_turn = TurnId::new();
    let survivor_turn = TurnId::new();
    state
        .turns
        .try_register(doomed, doomed_turn, ark_agent::CancelToken::new())
        .expect("register doomed");
    state
        .turns
        .try_register(survivor, survivor_turn, ark_agent::CancelToken::new())
        .expect("register survivor");
    // `remove` refuses while a turn is running, so settle both registrations.
    state.turns.finish(doomed_turn, &crate::TurnEnd::Completed);
    state
        .turns
        .finish(survivor_turn, &crate::TurnEnd::Completed);

    assert_eq!(state.turns.latest_for(doomed), Some(doomed_turn));
    assert_eq!(state.turns.latest_for(survivor), Some(survivor_turn));

    let status = remove(State(state.clone()), Path(doomed.to_string()))
        .await
        .expect("remove succeeds");
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        state.turns.latest_for(doomed),
        None,
        "a durable delete must reclaim the leaked epoch marker"
    );
    assert_eq!(
        state.turns.latest_for(survivor),
        Some(survivor_turn),
        "other threads keep their epoch marker"
    );
}

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
