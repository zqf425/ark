//! Thread list, pagination and projection tests.
//!
//! Split out of `threads/tests.rs` (AGENTS §4 file-size limit); declared as a
//! descendant of that module via `#[path]`, so `use super::*` still reaches
//! the shared doubles (`state`, `test_dir`, `NoopRunner`, …) exactly as when
//! everything lived in one file.

use super::*;

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
