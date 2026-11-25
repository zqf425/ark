use std::path::Path;
use std::sync::Arc;

use ark_common::{Result, Usage};
use ark_config::{ConfigStore, RuntimeConfig};
use ark_obs::UsageRecorder;
use ark_ports::{SessionStore, ToolRegistryPort};
use ark_store::{FileSessionStore, StoreConfig, StoreLimits};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use tower::ServiceExt;

use super::*;
use crate::api::routes::router;
use crate::state::{AppState, TurnEnd, TurnJob, TurnRunner};

struct TempDir(tempfile::TempDir);

impl TempDir {
    fn new() -> Self {
        Self(
            tempfile::Builder::new()
                .prefix("ark-diagnostics-test-")
                .tempdir()
                .expect("create test directory"),
        )
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

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

struct PendingRunner;

#[async_trait::async_trait]
impl TurnRunner for PendingRunner {
    async fn run_turn(&self, _job: TurnJob) -> Result<TurnEnd> {
        std::future::pending().await
    }
}

fn app(dir: &TempDir) -> (axum::Router, Arc<FileSessionStore>) {
    let store = Arc::new(
        FileSessionStore::open(StoreConfig::new(dir.path()).with_limits(StoreLimits::for_tests()))
            .expect("open test store"),
    );
    let state = AppState::new(
        store.clone(),
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig::default())),
        Arc::new(UsageRecorder::new()),
        Arc::new(PendingRunner),
        dir.path().to_path_buf(),
        "test",
    );
    (router(state), store)
}

fn at(offset_ms: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::milliseconds(offset_ms)
}

fn started(seq: u64, thread: ThreadId, turn: TurnId, at_ms: i64, recovery: u32) -> RuntimeEvent {
    RuntimeEvent::TurnStarted {
        auto_recovery: recovery,
        seq,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(at_ms),
        auto_continuation: false,
        attachment_ids: Vec::new(),
        attachment_outcomes: Vec::new(),
    }
}

fn completed(seq: u64, thread: ThreadId, turn: TurnId, at_ms: i64) -> RuntimeEvent {
    RuntimeEvent::TurnCompleted {
        seq,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(at_ms),
    }
}

fn usage_at(seq: u64, thread: ThreadId, turn: TurnId, at_ms: i64) -> RuntimeEvent {
    RuntimeEvent::Usage {
        seq,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(at_ms),
        usage: Usage::new(10, 5),
    }
}

#[test]
fn active_ms_equals_wall_clock_when_no_gate_opened() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0, 0),
        usage_at(2, thread, turn, 100),
        completed(3, thread, turn, 5_000),
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.last_seq, Some(3));
    assert_eq!(response.turn_usage[0].active_ms, Some(5_000));
    assert_eq!(response.turn_usage[0].gate_suspensions, Some(0));
    assert_eq!(response.active_ms, Some(5_000));
}

#[test]
fn active_ms_excludes_the_user_gate_window() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0, 0),
        usage_at(2, thread, turn, 1_000),
        RuntimeEvent::ApprovalRequested {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(2_000),
            approval_id: "a1".into(),
            tool: "bash".into(),
            summary: "cargo test".into(),
        },
        RuntimeEvent::ApprovalResolved {
            seq: 4,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(2_000 + 3_600_000),
            approval_id: "a1".into(),
            approved: true,
        },
        usage_at(5, thread, turn, 2_000 + 3_600_000 + 500),
        completed(6, thread, turn, 2_000 + 3_600_000 + 1_500),
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turn_usage[0].active_ms, Some(3_500));
    assert_eq!(response.turn_usage[0].gate_suspensions, Some(1));
    assert_eq!(response.active_ms, Some(3_500));
}

#[test]
fn user_input_gate_freezes_the_clock_too() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0, 0),
        RuntimeEvent::UserInputRequested {
            seq: 2,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(500),
            request_id: "u1".into(),
            prompt: "which db?".into(),
            item_id: None,
        },
        RuntimeEvent::UserInputResolved {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(62_500),
            request_id: "u1".into(),
            answer: "postgres".into(),
            item_id: None,
        },
        completed(4, thread, turn, 63_000),
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turn_usage[0].active_ms, Some(1_000));
    assert_eq!(response.turn_usage[0].gate_suspensions, Some(1));
}

#[test]
fn running_turn_extends_to_the_newest_event() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0, 0),
        usage_at(2, thread, turn, 2_500),
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turn_usage[0].active_ms, Some(2_500));
}

#[test]
fn idle_gap_before_a_user_turn_is_not_counted() {
    let thread = ThreadId::new();
    let turn_a = TurnId::new();
    let turn_b = TurnId::new();
    let events = vec![
        started(1, thread, turn_a, 0, 0),
        completed(2, thread, turn_a, 1_000),
        started(3, thread, turn_b, 1_000 + 1_800_000, 0),
        completed(4, thread, turn_b, 1_000 + 1_800_000 + 2_000),
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turn_usage[0].active_ms, Some(1_000));
    assert_eq!(response.turn_usage[1].active_ms, Some(2_000));
    assert_eq!(response.active_ms, Some(3_000));
}

#[test]
fn auto_recovery_backoff_rides_the_recovery_turn_span() {
    let thread = ThreadId::new();
    let turn_a = TurnId::new();
    let turn_b = TurnId::new();
    let events = vec![
        started(1, thread, turn_a, 0, 0),
        RuntimeEvent::TurnFailed {
            seq: 2,
            thread_id: thread,
            turn_id: turn_a,
            timestamp: at(1_000),
            message: "502".into(),
            model_request_failed: true,
            item_id: None,
        },
        started(3, thread, turn_b, 31_000, 1),
        completed(4, thread, turn_b, 33_000),
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turn_usage[0].active_ms, Some(1_000));
    assert_eq!(response.turn_usage[1].active_ms, Some(32_000));
    assert_eq!(response.active_ms, Some(33_000));
}

#[test]
fn auto_continuation_gap_is_counted_like_recovery() {
    let thread = ThreadId::new();
    let turn_a = TurnId::new();
    let turn_b = TurnId::new();
    let events = vec![
        started(1, thread, turn_a, 0, 0),
        completed(2, thread, turn_a, 1_000),
        RuntimeEvent::TurnStarted {
            auto_recovery: 0,
            seq: 3,
            thread_id: thread,
            turn_id: turn_b,
            timestamp: at(11_000),
            auto_continuation: true,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        },
        completed(4, thread, turn_b, 12_000),
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turn_usage[1].active_ms, Some(11_000));
    assert_eq!(response.active_ms, Some(12_000));
}

#[test]
fn a_turn_without_usage_still_reports_its_active_span() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0, 0),
        usage_at(2, thread, turn, 800),
        RuntimeEvent::TurnFailed {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(1_200),
            message: "boom".into(),
            model_request_failed: false,
            item_id: None,
        },
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turn_usage.len(), 1);
    assert_eq!(response.turn_usage[0].turn_id, turn);
    assert_eq!(response.turn_usage[0].active_ms, Some(1_200));
    assert_eq!(response.samples, 1);
}

#[test]
fn gate_open_at_terminal_counts_a_suspension_without_freezing() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 0, 0),
        RuntimeEvent::ApprovalRequested {
            seq: 2,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(500),
            approval_id: "a1".into(),
            tool: "bash".into(),
            summary: "x".into(),
        },
        RuntimeEvent::TurnAborted {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(600),
            reason: "user interrupt".into(),
        },
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turn_usage[0].active_ms, Some(500));
    assert_eq!(response.turn_usage[0].gate_suspensions, Some(1));
}

#[test]
fn clock_skew_never_pushes_active_ms_negative() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        started(1, thread, turn, 1_000, 0),
        RuntimeEvent::ApprovalRequested {
            seq: 2,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(2_000),
            approval_id: "a1".into(),
            tool: "bash".into(),
            summary: "x".into(),
        },
        RuntimeEvent::ApprovalResolved {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(1_500),
            approval_id: "a1".into(),
            approved: true,
        },
        completed(4, thread, turn, 2_500),
    ];
    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turn_usage[0].active_ms, Some(1_500));
    assert_eq!(response.turn_usage[0].gate_suspensions, Some(1));
}

#[test]
fn thread_usage_aggregates_cache_counters_across_turns() {
    let thread = ThreadId::new();
    let turn_a = TurnId::new();
    let turn_b = TurnId::new();
    let events = vec![
        RuntimeEvent::TurnStarted {
            auto_recovery: 0,
            seq: 1,
            thread_id: thread,
            turn_id: turn_a,
            timestamp: at(0),
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        },
        RuntimeEvent::Usage {
            seq: 2,
            thread_id: thread,
            turn_id: turn_a,
            timestamp: at(10),
            usage: Usage::new(100, 20).with_cache(Some(80), Some(20)),
        },
        RuntimeEvent::TurnStarted {
            auto_recovery: 0,
            seq: 3,
            thread_id: thread,
            turn_id: turn_b,
            timestamp: at(20),
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        },
        RuntimeEvent::Usage {
            seq: 4,
            thread_id: thread,
            turn_id: turn_b,
            timestamp: at(30),
            usage: Usage::new(50, 10),
        },
    ];

    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.thread_id, thread);
    assert_eq!(response.turns, 2);
    assert_eq!(response.samples, 2);
    assert_eq!(response.usage.prompt_tokens, 150);
    assert_eq!(response.usage.completion_tokens, 30);
    assert_eq!(response.usage.cache_hit_tokens, Some(80));
    assert_eq!(response.usage.cache_hit_rate, None);
    assert_eq!(response.turn_usage.len(), 2);
    assert_eq!(response.turn_usage[0].turn_id, turn_a);
    assert_eq!(response.turn_usage[0].samples, 1);
    assert_eq!(response.turn_usage[1].turn_id, turn_b);
    let folded_prompt: u64 = response
        .turn_usage
        .iter()
        .map(|turn| turn.usage.prompt_tokens)
        .sum();
    assert_eq!(folded_prompt, response.usage.prompt_tokens);
}

#[test]
fn per_turn_usage_keeps_every_turn_regardless_of_page_bounds() {
    let thread = ThreadId::new();
    let mut events = Vec::new();
    let mut turns = Vec::new();
    for index in 0..40u64 {
        let turn = TurnId::new();
        turns.push(turn);
        events.push(RuntimeEvent::TurnStarted {
            auto_recovery: 0,
            seq: index * 4,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(index as i64 * 100),
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        });
        for round in 0..2u64 {
            let mut usage = Usage::new(10, 1);
            usage.duration_ms = Some(1_500);
            events.push(RuntimeEvent::Usage {
                seq: index * 4 + round + 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(index as i64 * 100 + round as i64 + 1),
                usage,
            });
        }
    }

    let response = aggregate_thread_usage(thread, &events);
    assert_eq!(response.turns, 40);
    assert_eq!(response.turn_usage.len(), 40);
    let last = response.turn_usage.last().expect("newest turn");
    assert_eq!(last.turn_id, *turns.last().unwrap());
    assert_eq!(last.samples, 2);
    assert_eq!(last.usage.duration_ms, Some(3_000));
    let folded: u64 = response
        .turn_usage
        .iter()
        .map(|turn| turn.usage.duration_ms.unwrap_or(0))
        .sum();
    assert_eq!(Some(folded), response.usage.duration_ms);
}

#[test]
fn rounds_cover_the_whole_call_window_and_attach_errors() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![
        RuntimeEvent::TurnStarted {
            auto_recovery: 0,
            seq: 1,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(0),
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        },
        RuntimeEvent::Error {
            seq: 2,
            thread_id: thread,
            timestamp: at(5),
            code: ArkErrorCode::ModelRequestFailed,
            message: "provider hiccup".into(),
        },
        RuntimeEvent::Usage {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(10),
            usage: Usage::new(10, 5),
        },
        RuntimeEvent::Usage {
            seq: 4,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(40),
            usage: Usage::new(20, 10),
        },
        RuntimeEvent::Error {
            seq: 5,
            thread_id: thread,
            timestamp: at(45),
            code: ArkErrorCode::ModelRequestFailed,
            message: "late failure".into(),
        },
        RuntimeEvent::TurnCompleted {
            seq: 6,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(50),
        },
    ];

    let rounds = derive_rounds(&events, None);
    assert_eq!(rounds.len(), 2);
    assert_eq!(rounds[0].round, 1);
    assert_eq!(rounds[0].started_at, at(0));
    assert_eq!(rounds[0].ended_at, at(10));
    assert_eq!(rounds[0].usage.duration_ms, Some(10));
    assert_eq!(rounds[0].errors.len(), 1);
    assert_eq!(rounds[0].errors[0].message, "provider hiccup");
    assert_eq!(rounds[1].round, 2);
    assert_eq!(rounds[1].started_at, at(10));
    assert_eq!(rounds[1].usage.duration_ms, Some(30));
    assert_eq!(rounds[1].errors.len(), 1);
    assert_eq!(rounds[1].errors[0].message, "late failure");
}

#[test]
fn measured_round_duration_wins_over_the_boundary_window() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let mut measured = Usage::new(10, 5);
    measured.duration_ms = Some(1_200);
    let events = vec![
        RuntimeEvent::TurnStarted {
            auto_recovery: 0,
            seq: 1,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(0),
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        },
        RuntimeEvent::Usage {
            seq: 2,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(10),
            usage: measured,
        },
        RuntimeEvent::Usage {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(40),
            usage: Usage::new(20, 10),
        },
    ];

    let rounds = derive_rounds(&events, None);
    assert_eq!(rounds.len(), 2);
    assert_eq!(rounds[0].usage.duration_ms, Some(1_200));
    assert_eq!(rounds[1].usage.duration_ms, Some(30));
}

#[test]
fn a_turn_without_usage_produces_no_round() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let events = vec![RuntimeEvent::TurnStarted {
        auto_recovery: 0,
        seq: 1,
        thread_id: thread,
        turn_id: turn,
        timestamp: at(0),
        auto_continuation: false,
        attachment_ids: Vec::new(),
        attachment_outcomes: Vec::new(),
    }];
    assert!(derive_rounds(&events, None).is_empty());
}

#[tokio::test]
async fn usage_route_aggregates_from_the_event_log() {
    let dir = TempDir::new();
    let (app, store) = app(&dir);
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/threads")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let thread: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let thread_id: ThreadId = thread["thread_id"]
        .as_str()
        .expect("thread id")
        .parse()
        .expect("parse thread id");

    let turn = TurnId::new();
    store
        .append_event(
            thread_id,
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 0,
                thread_id,
                turn_id: turn,
                timestamp: Utc::now(),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
        )
        .await
        .unwrap();
    store
        .append_event(
            thread_id,
            RuntimeEvent::Usage {
                seq: 0,
                thread_id,
                turn_id: turn,
                timestamp: Utc::now(),
                usage: Usage::new(120, 30).with_cache(Some(90), Some(30)),
            },
        )
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/threads/{thread_id}/usage"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["thread_id"], thread_id.to_string());
    assert_eq!(json["turns"], 1);
    assert_eq!(json["samples"], 1);
    assert_eq!(json["prompt_tokens"], 120);
    assert_eq!(json["cache_hit_tokens"], 90);
}

#[tokio::test]
async fn llm_rounds_route_is_bounded_and_filterable() {
    let dir = TempDir::new();
    let (app, store) = app(&dir);
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/threads")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let thread: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let thread_id: ThreadId = thread["thread_id"]
        .as_str()
        .expect("thread id")
        .parse()
        .expect("parse thread id");

    for round_no in 0..3u64 {
        let turn = TurnId::new();
        store
            .append_event(
                thread_id,
                RuntimeEvent::TurnStarted {
                    auto_recovery: 0,
                    seq: 0,
                    thread_id,
                    turn_id: turn,
                    timestamp: at(round_no as i64 * 100),
                    auto_continuation: false,
                    attachment_ids: Vec::new(),
                    attachment_outcomes: Vec::new(),
                },
            )
            .await
            .unwrap();
        store
            .append_event(
                thread_id,
                RuntimeEvent::Usage {
                    seq: 0,
                    thread_id,
                    turn_id: turn,
                    timestamp: at(round_no as i64 * 100 + 50),
                    usage: Usage::new(10, 1),
                },
            )
            .await
            .unwrap();
    }

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/debug/llm-rounds?limit=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(json["truncated"], true);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/debug/llm-rounds?thread_id={thread_id}&limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["rounds"].as_array().unwrap().len(), 3);
    assert_eq!(json["truncated"], false);
}

#[tokio::test]
async fn unknown_thread_answers_structured_not_found() {
    let dir = TempDir::new();
    let (app, _store) = app(&dir);
    let missing = ThreadId::new();
    for uri in [
        format!("/v1/threads/{missing}/usage"),
        format!("/v1/debug/llm-rounds?thread_id={missing}"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
