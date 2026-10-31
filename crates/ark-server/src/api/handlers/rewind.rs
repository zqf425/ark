//! `POST /v1/threads/{id}/rewind` physically truncates durable history.

use axum::extract::{Path, State};
use axum::Json;

use ark_common::ArkErrorCode;
use ark_contract::{RewindRequest, RewindResponse, RuntimeEvent};

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::state::AppState;

/// Rewind a completed thread to immediately before a sequence or turn.
pub async fn rewind(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<RewindRequest>,
) -> Result<Json<RewindResponse>, ApiError> {
    let thread_id = parse_thread(&id)?;
    let _lifecycle_guard = state.workspace_gate.lock().await;
    if state.turns.active_for(thread_id).is_some() {
        return Err(ApiError::new(
            ArkErrorCode::Conflict,
            "cannot rewind a thread with a running turn",
        ));
    }
    match target(&state, thread_id, request).await? {
        Target::New { seq, turn_id } => Ok(Json(
            state.store.rewind_thread(thread_id, seq, turn_id).await?,
        )),
        Target::Existing { seq, last_seq } => Ok(Json(RewindResponse {
            last_seq,
            rewind_from_seq: seq,
            already_rewound: true,
        })),
    }
}

enum Target {
    New {
        seq: u64,
        turn_id: Option<ark_common::TurnId>,
    },
    Existing {
        seq: u64,
        last_seq: u64,
    },
}

async fn target(
    state: &AppState,
    thread_id: ark_common::ThreadId,
    request: RewindRequest,
) -> Result<Target, ApiError> {
    if request.turn_id.is_some() == request.seq.is_some() {
        return Err(ApiError::new(
            ArkErrorCode::ValidationError,
            "provide exactly one of turn_id or seq",
        ));
    }
    let events = state.store.replay_events(thread_id, 0).await?;
    if let Some(turn_id) = request.turn_id {
        if let Some(seq) = events.iter().find_map(|event| match event {
            RuntimeEvent::TurnStarted {
                seq,
                turn_id: found,
                ..
            } if *found == turn_id => Some(*seq),
            _ => None,
        }) {
            return Ok(Target::New {
                seq,
                turn_id: Some(turn_id),
            });
        }
        if let Some(seq) = events.iter().find_map(|event| match event {
            RuntimeEvent::ThreadRewound {
                rewind_from_seq,
                target_turn_id: Some(found),
                ..
            } if *found == turn_id => Some(*rewind_from_seq),
            _ => None,
        }) {
            return Ok(Target::Existing {
                seq,
                last_seq: state.store.highest_seq(thread_id).await?,
            });
        }
        if let Some(response) = state
            .store
            .rewind_record(thread_id, None, Some(turn_id))
            .await?
        {
            return Ok(Target::Existing {
                seq: response.rewind_from_seq,
                last_seq: response.last_seq,
            });
        }
        return Err(ApiError::new(
            ArkErrorCode::NotFound,
            "rewind target turn does not exist",
        ));
    }
    let Some(seq) = request.seq else {
        return Err(ApiError::new(
            ArkErrorCode::ValidationError,
            "missing rewind target",
        ));
    };
    if events.iter().any(|event| event.seq() == seq) {
        return Ok(Target::New { seq, turn_id: None });
    }
    if events.iter().any(|event| matches!(event, RuntimeEvent::ThreadRewound { rewind_from_seq, .. } if *rewind_from_seq == seq)) {
        return Ok(Target::Existing { seq, last_seq: state.store.highest_seq(thread_id).await? });
    }
    if let Some(response) = state
        .store
        .rewind_record(thread_id, Some(seq), None)
        .await?
    {
        return Ok(Target::Existing {
            seq,
            last_seq: response.last_seq,
        });
    }
    let last_seq = state.store.highest_seq(thread_id).await?;
    let message = if seq > last_seq {
        "rewind target is later than the current thread tail"
    } else {
        "rewind target sequence does not exist"
    };
    Err(ApiError::new(ArkErrorCode::NotFound, message))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use ark_agent::CancelToken;
    use ark_common::{Result, ThreadId, TurnId};
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_contract::{item::ItemKind, RuntimeEvent, ThreadItem};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig, StoreLimits};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::Utc;
    use tower::ServiceExt;

    use super::*;
    use crate::api::routes::router;
    use crate::state::{TurnEnd, TurnJob, TurnRunner};

    /// Backed by `tempfile`, whose random name keeps two parallel tests from
    /// picking the same path.
    struct TempDir(tempfile::TempDir);

    impl TempDir {
        fn new() -> Self {
            let dir = tempfile::Builder::new()
                .prefix("ark-rewind-test-")
                .tempdir()
                .expect("create test directory");
            Self(dir)
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

    fn app(dir: &TempDir) -> (axum::Router, Arc<FileSessionStore>, AppState) {
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
        (router(state.clone()), store, state)
    }

    async fn thread_with_two_turns(store: &Arc<FileSessionStore>) -> (ThreadId, TurnId, u64) {
        let thread = store
            .create_thread("rewind")
            .await
            .expect("create thread")
            .thread_id;
        let retained_turn = TurnId::new();
        let discarded_turn = TurnId::new();
        for event in [
            RuntimeEvent::TurnStarted {
                seq: 0,
                thread_id: thread,
                turn_id: retained_turn,
                timestamp: Utc::now(),
                auto_continuation: false,
            },
            RuntimeEvent::ItemCreated {
                seq: 0,
                thread_id: thread,
                turn_id: retained_turn,
                timestamp: Utc::now(),
                item: ThreadItem::new(retained_turn, ItemKind::UserMessage)
                    .with_text("keep")
                    .completed(),
            },
            RuntimeEvent::TurnCompleted {
                seq: 0,
                thread_id: thread,
                turn_id: retained_turn,
                timestamp: Utc::now(),
            },
            RuntimeEvent::TurnStarted {
                seq: 0,
                thread_id: thread,
                turn_id: discarded_turn,
                timestamp: Utc::now(),
                auto_continuation: false,
            },
            RuntimeEvent::ItemCreated {
                seq: 0,
                thread_id: thread,
                turn_id: discarded_turn,
                timestamp: Utc::now(),
                item: ThreadItem::new(discarded_turn, ItemKind::UserMessage)
                    .with_text("discard")
                    .completed(),
            },
            RuntimeEvent::TurnCompleted {
                seq: 0,
                thread_id: thread,
                turn_id: discarded_turn,
                timestamp: Utc::now(),
            },
        ] {
            store
                .append_event(thread, event)
                .await
                .expect("append event");
        }
        (thread, discarded_turn, 4)
    }

    fn rewind_request(thread: ThreadId, turn: TurnId) -> Request<Body> {
        Request::post(format!("/v1/threads/{thread}/rewind"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "turn_id": turn }).to_string(),
            ))
            .expect("build rewind request")
    }

    #[tokio::test]
    async fn rewind_detail_omits_items_from_the_discarded_turn() {
        let dir = TempDir::new();
        let (app, store, _) = app(&dir);
        let (thread, discarded_turn, _) = thread_with_two_turns(&store).await;

        let response = app
            .clone()
            .oneshot(rewind_request(thread, discarded_turn))
            .await
            .expect("rewind response");
        assert_eq!(response.status(), StatusCode::OK);
        let detail = app
            .oneshot(
                Request::get(format!("/v1/threads/{thread}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("detail response");
        assert_eq!(detail.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(detail.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("discard"));
    }

    #[tokio::test]
    async fn repeated_rewind_of_the_same_target_is_idempotent() {
        let dir = TempDir::new();
        let (app, store, _) = app(&dir);
        let (thread, discarded_turn, _) = thread_with_two_turns(&store).await;

        let first = app
            .clone()
            .oneshot(rewind_request(thread, discarded_turn))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let second = app
            .oneshot(rewind_request(thread, discarded_turn))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejected_subagent_configuration_returns_apply_result() {
        let dir = TempDir::new();
        let (app, _, _) = app(&dir);
        let request = Request::post("/v1/subagents/config")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"enabled":true,"max_concurrency":9}"#))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["result"],
            "rejected"
        );
    }

    #[tokio::test]
    async fn rewind_of_a_thread_with_a_running_turn_is_conflict() {
        let dir = TempDir::new();
        let (app, store, state) = app(&dir);
        let (thread, discarded_turn, _) = thread_with_two_turns(&store).await;
        state
            .turns
            .try_register(thread, TurnId::new(), CancelToken::new())
            .expect("register running turn");

        let response = app
            .oneshot(rewind_request(thread, discarded_turn))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
}
