//! Thread detail runtime-override and child-id projection tests.
//!
//! Split out of `threads/tests.rs` (AGENTS §4 file-size limit); declared as a
//! descendant of that module via `#[path]`, so `use super::*` still reaches
//! the shared doubles (`state`, `test_dir`, `NoopRunner`, …) exactly as when
//! everything lived in one file.

use super::*;

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
