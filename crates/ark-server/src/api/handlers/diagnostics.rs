//! Per-thread usage aggregation and LLM round diagnostics.
//!
//! Both projections derive from the authoritative event log: the agent loop
//! appends exactly one `RuntimeEvent::Usage` per completed model round, so a
//! round boundary, the per-thread totals, and the cache counters all come from
//! the same source the renderer replays. Nothing here keeps a second copy of
//! usage state, which is what keeps `GET /v1/usage`, the per-thread route, and
//! `GET /v1/debug/llm-rounds` consistent across restarts.

use std::collections::{HashMap, HashSet};

use ark_common::{ArkErrorCode, ThreadId, TurnId, Usage};
use ark_contract::{
    LlmRound, LlmRoundError, LlmRoundsResponse, RuntimeEvent, ThreadUsageResponse,
    LLM_ROUNDS_DEFAULT_LIMIT, LLM_ROUNDS_MAX_LIMIT, LLM_ROUNDS_MAX_THREADS,
};
use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::state::AppState;

/// `GET /v1/threads/{id}/usage`.
///
/// Aggregated from the thread's persisted `usage` events, so it survives
/// restarts and reports exactly what the provider sent, including native cache
/// counters when present.
pub async fn thread_usage(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadUsageResponse>, ApiError> {
    let thread = parse_thread(&id)?;
    state.store.get_thread(thread).await?;
    let events = state.store.replay_events(thread, 0).await?;
    Ok(Json(aggregate_thread_usage(thread, &events)))
}

/// `GET /v1/debug/llm-rounds`.
///
/// Bounded on purpose: `limit` clamps to `[0, LLM_ROUNDS_MAX_LIMIT]`, and
/// without a `thread_id` the scan covers at most `LLM_ROUNDS_MAX_THREADS`
/// most recently updated threads, so the route can never walk the whole store.
pub async fn llm_rounds(
    State(state): State<AppState>,
    Query(query): Query<LlmRoundsQuery>,
) -> Result<Json<LlmRoundsResponse>, ApiError> {
    let limit = query
        .limit
        .unwrap_or(LLM_ROUNDS_DEFAULT_LIMIT)
        .min(LLM_ROUNDS_MAX_LIMIT) as usize;
    let offset = query.offset.unwrap_or(0) as usize;
    let turn_filter = match query.turn_id.as_deref() {
        Some(raw) => Some(parse_turn(raw)?),
        None => None,
    };

    let threads: Vec<ThreadId> = match query.thread_id.as_deref() {
        Some(raw) => {
            let thread = parse_thread(raw)?;
            // Fail fast on an unknown thread instead of answering empty rounds.
            state.store.get_thread(thread).await?;
            vec![thread]
        }
        None => state
            .store
            .list_threads()
            .await?
            .into_iter()
            .take(LLM_ROUNDS_MAX_THREADS)
            .map(|meta| meta.thread_id)
            .collect(),
    };

    let mut rounds = Vec::new();
    for thread in threads {
        let events = state.store.replay_events(thread, 0).await?;
        rounds.extend(derive_rounds(&events, turn_filter));
    }
    rounds.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.thread_id.to_string().cmp(&b.thread_id.to_string()))
            .then_with(|| a.turn_id.to_string().cmp(&b.turn_id.to_string()))
            .then_with(|| a.round.cmp(&b.round))
    });
    let truncated = rounds.len() > offset.saturating_add(limit);
    let rounds = rounds.into_iter().skip(offset).take(limit).collect();
    Ok(Json(LlmRoundsResponse { rounds, truncated }))
}

#[derive(Debug, Deserialize)]
pub struct LlmRoundsQuery {
    /// Restrict the diagnostics to one thread.
    #[serde(default)]
    pub thread_id: Option<String>,
    /// Further restrict to one turn inside `thread_id`.
    #[serde(default)]
    pub turn_id: Option<String>,
    /// Page size, clamped to `[0, LLM_ROUNDS_MAX_LIMIT]` when present.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Zero-based offset into the deterministic round ordering.
    #[serde(default)]
    pub offset: Option<u32>,
}

fn parse_turn(raw: &str) -> Result<TurnId, ApiError> {
    raw.parse::<TurnId>().map_err(|err| {
        ApiError::new(
            ArkErrorCode::ValidationError,
            format!("`{raw}` is not a valid turn id: {err}"),
        )
    })
}

/// Fold every persisted `usage` event of one thread into the wire totals.
fn aggregate_thread_usage(thread: ThreadId, events: &[RuntimeEvent]) -> ThreadUsageResponse {
    let mut turns: HashSet<TurnId> = HashSet::new();
    let mut samples = 0u64;
    let mut usage = ark_common::Usage::default();
    for event in events {
        if let RuntimeEvent::Usage {
            turn_id,
            usage: sample,
            ..
        } = event
        {
            turns.insert(*turn_id);
            samples += 1;
            usage.merge(sample);
        }
    }
    ThreadUsageResponse {
        thread_id: thread,
        turns: turns.len() as u64,
        samples,
        usage,
    }
}

/// Per-turn accumulator for [`derive_rounds`].
struct TurnAccum {
    started_at: Option<DateTime<Utc>>,
    last_end: Option<DateTime<Utc>>,
    round: u64,
    pending_errors: Vec<(DateTime<Utc>, LlmRoundError)>,
    rounds: Vec<LlmRound>,
}

/// Close one model round: drain the errors recorded inside its window, defer
/// anything later, and push the finished round onto the accumulator.
fn close_round(
    acc: &mut TurnAccum,
    thread_id: ThreadId,
    turn_id: TurnId,
    ended_at: DateTime<Utc>,
    usage: &Usage,
) {
    let started_at = acc.last_end.or(acc.started_at).unwrap_or(ended_at);
    acc.round += 1;
    let round = acc.round;
    let mut errors = Vec::new();
    let mut deferred = Vec::new();
    for (at, error) in acc.pending_errors.drain(..) {
        if at <= ended_at {
            errors.push(error);
        } else {
            deferred.push((at, error));
        }
    }
    acc.pending_errors = deferred;
    acc.rounds.push(LlmRound {
        thread_id,
        turn_id,
        round,
        started_at,
        ended_at,
        duration_ms: (ended_at - started_at).num_milliseconds().max(0) as u64,
        errors,
        usage: usage.clone(),
    });
    acc.last_end = Some(ended_at);
}

/// Flush the errors a turn still holds when it ends.
///
/// Errors recorded after the final round still belong to that turn's last
/// round, because they happened while the runtime was working on it. A turn
/// without rounds simply drops its pending errors.
fn flush_turn_errors(acc: &mut TurnAccum) {
    if let Some(last) = acc.rounds.last_mut() {
        for (_, error) in acc.pending_errors.drain(..) {
            last.errors.push(error);
        }
    } else {
        acc.pending_errors.clear();
    }
}

/// Derive completed model rounds from the authoritative event log.
///
/// A round closes on the `Usage` event the loop appends right after a model
/// call settles; its window starts at the previous boundary inside the same
/// turn (the previous round's end, or `turn_started` for the first round).
/// Errors recorded inside a round's window attach to that round; errors after
/// the final round of a turn attach to that last round. A model call that dies
/// before settling produces no `Usage` event and therefore no round - the turn
/// failure stays visible through `TurnFailed`/`Error` events on the thread.
fn derive_rounds(events: &[RuntimeEvent], turn_filter: Option<TurnId>) -> Vec<LlmRound> {
    let mut turns: HashMap<TurnId, TurnAccum> = HashMap::new();
    let mut current: Option<TurnId> = None;

    for event in events {
        match event {
            RuntimeEvent::TurnStarted {
                turn_id, timestamp, ..
            } => {
                current = Some(*turn_id);
                turns.insert(
                    *turn_id,
                    TurnAccum {
                        started_at: Some(*timestamp),
                        last_end: None,
                        round: 0,
                        pending_errors: Vec::new(),
                        rounds: Vec::new(),
                    },
                );
            }
            RuntimeEvent::Usage {
                thread_id,
                turn_id,
                timestamp,
                usage,
                ..
            } => {
                if let Some(acc) = turns.get_mut(turn_id) {
                    close_round(acc, *thread_id, *turn_id, *timestamp, usage);
                }
            }
            RuntimeEvent::Error {
                timestamp,
                code,
                message,
                ..
            } => {
                if let Some(turn_id) = current {
                    if let Some(acc) = turns.get_mut(&turn_id) {
                        acc.pending_errors.push((
                            *timestamp,
                            LlmRoundError {
                                code: code.clone(),
                                message: message.clone(),
                            },
                        ));
                    }
                }
            }
            RuntimeEvent::TurnCompleted { turn_id, .. }
            | RuntimeEvent::TurnFailed { turn_id, .. }
            | RuntimeEvent::TurnAborted { turn_id, .. } => {
                if let Some(acc) = turns.get_mut(turn_id) {
                    flush_turn_errors(acc);
                }
                if current == Some(*turn_id) {
                    current = None;
                }
            }
            _ => {}
        }
    }

    let mut all: Vec<LlmRound> = Vec::new();
    for acc in turns.into_values() {
        all.extend(acc.rounds);
    }
    if let Some(filter) = turn_filter {
        all.retain(|round| round.turn_id == filter);
    }
    all
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use ark_agent::CancelToken;
    use ark_common::{Result, Usage};
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_contract::item::ItemKind;
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

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ark-diagnostics-test-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().expect("system clock")
            ));
            std::fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
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
            FileSessionStore::open(
                StoreConfig::new(dir.path()).with_limits(StoreLimits::for_tests()),
            )
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
        // Deterministic base so every call in a fixture shares one timeline.
        DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::milliseconds(offset_ms)
    }

    #[test]
    fn thread_usage_aggregates_cache_counters_across_turns() {
        let thread = ThreadId::new();
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let events = vec![
            RuntimeEvent::TurnStarted {
                seq: 1,
                thread_id: thread,
                turn_id: turn_a,
                timestamp: at(0),
            },
            RuntimeEvent::Usage {
                seq: 2,
                thread_id: thread,
                turn_id: turn_a,
                timestamp: at(10),
                usage: Usage::new(100, 20).with_cache(Some(80), Some(20)),
            },
            RuntimeEvent::TurnStarted {
                seq: 3,
                thread_id: thread,
                turn_id: turn_b,
                timestamp: at(20),
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
        // One sample without cache counters keeps the aggregated rate unknown
        // instead of inventing a definite hit rate from mixed evidence.
        assert_eq!(response.usage.cache_hit_tokens, Some(80));
        assert_eq!(response.usage.cache_hit_rate, None);
    }

    #[test]
    fn rounds_cover_the_whole_call_window_and_attach_errors() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![
            RuntimeEvent::TurnStarted {
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: at(0),
            },
            // An error while the first model call is still in flight.
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
            // After the final round: attached to the last round at turn end.
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
        assert_eq!(rounds[0].duration_ms, 10);
        assert_eq!(rounds[0].errors.len(), 1);
        assert_eq!(rounds[0].errors[0].message, "provider hiccup");
        assert_eq!(rounds[1].round, 2);
        assert_eq!(rounds[1].started_at, at(10));
        assert_eq!(rounds[1].duration_ms, 30);
        assert_eq!(rounds[1].errors.len(), 1);
        assert_eq!(rounds[1].errors[0].message, "late failure");
    }

    #[test]
    fn a_turn_without_usage_produces_no_round() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let events = vec![RuntimeEvent::TurnStarted {
            seq: 1,
            thread_id: thread,
            turn_id: turn,
            timestamp: at(0),
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
                    seq: 0,
                    thread_id,
                    turn_id: turn,
                    timestamp: Utc::now(),
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
                        seq: 0,
                        thread_id,
                        turn_id: turn,
                        timestamp: at(round_no as i64 * 100),
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

        // limit=2 keeps the route bounded.
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

        // thread filter resolves the same rounds for the explicit thread.
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

    // The unused `event` helper is kept out of the build; drop the stub that
    // referenced it instead of fighting the type inference of a generic fn path.
}
