//! Session lifecycle routes.
//!
//! The runtime has one durable conversation object: the thread. `POST
//! /v1/sessions/{id}/resume-thread` therefore accepts the session id as the
//! thread id itself - a compat adapter, not a second session data model.
//! Resuming never creates anything implicitly: an unknown id is a structured
//! 404 and a corrupt store error surfaces as its structured code.

use ark_contract::SessionResumeResponse;
use axum::extract::{Path, State};
use axum::Json;

use crate::api::error::ApiError;
use crate::api::handlers::threads::{load_detail, parse_thread};
use crate::state::AppState;

/// `POST /v1/sessions/{id}/resume-thread`.
pub async fn resume_thread(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionResumeResponse>, ApiError> {
    let session_id = parse_thread(&id)?;
    let thread = load_detail(&state, session_id).await?;
    Ok(Json(SessionResumeResponse { session_id, thread }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use ark_common::Result;
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig, StoreLimits};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::api::routes::router;
    use crate::state::{AppState, TurnEnd, TurnJob, TurnRunner};

    /// Backed by `tempfile`, whose random name keeps two parallel tests from
    /// picking the same path.
    struct TempDir(tempfile::TempDir);

    impl TempDir {
        fn new() -> Self {
            let dir = tempfile::Builder::new()
                .prefix("ark-sessions-test-")
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

    async fn post_json(app: &axum::Router, uri: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn resume_thread_returns_the_thread_detail() {
        let dir = TempDir::new();
        let (app, store) = app(&dir);
        let created = store
            .create_thread("resume-me")
            .await
            .expect("create thread");
        let uri = format!("/v1/sessions/{}/resume-thread", created.thread_id);

        let response = post_json(&app, &uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["session_id"],
            serde_json::json!(created.thread_id.to_string())
        );
        assert_eq!(body["thread"]["summary"]["title"], "resume-me");
    }

    #[tokio::test]
    async fn resume_thread_answers_structured_not_found_for_unknown_sessions() {
        let dir = TempDir::new();
        let (app, _store) = app(&dir);
        let missing = ark_common::ThreadId::new();
        let response = post_json(&app, &format!("/v1/sessions/{missing}/resume-thread")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resume_thread_rejects_garbage_session_ids() {
        let dir = TempDir::new();
        let (app, _store) = app(&dir);
        let response = post_json(&app, "/v1/sessions/not-a-uuid/resume-thread").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
