//! `GET /v1/threads/{id}/export` - a versioned, deterministic thread snapshot.
//!
//! The export is built only from the durable store: summary metadata plus the
//! full authoritative event log in ascending `seq` order. It adds no export
//! timestamp (that would break determinism), carries no credentials, API keys,
//! headers, or attachment payloads, and replays byte-identically for identical
//! store state.

use ark_contract::{ThreadExport, THREAD_EXPORT_FORMAT_VERSION};
use axum::extract::{Path, State};
use axum::Json;

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::state::AppState;

/// `GET /v1/threads/{id}/export`.
pub async fn export(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadExport>, ApiError> {
    let thread_id = parse_thread(&id)?;
    let meta = state.store.get_thread(thread_id).await?;
    let events = state.store.replay_events(thread_id, 0).await?;
    Ok(Json(ThreadExport {
        format_version: THREAD_EXPORT_FORMAT_VERSION,
        thread_id: meta.thread_id,
        title: meta.title,
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        archived: meta.archived,
        runtime: meta.runtime,
        events,
    }))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use ark_common::Result;
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig, StoreLimits};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::Utc;
    use tower::ServiceExt;

    use crate::api::routes::router;
    use crate::state::{AppState, TurnEnd, TurnJob, TurnRunner};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ark-export-test-{}-{}",
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

    async fn get_json(app: &axum::Router, uri: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn export_is_versioned_and_deterministic() {
        let dir = TempDir::new();
        let (app, store) = app(&dir);
        let created = store
            .create_thread("export-me")
            .await
            .expect("create thread");
        store
            .append_event(
                created.thread_id,
                ark_contract::RuntimeEvent::TurnStarted {
                    seq: 0,
                    thread_id: created.thread_id,
                    turn_id: ark_common::TurnId::new(),
                    timestamp: Utc::now(),
                    auto_continuation: false,
                },
            )
            .await
            .expect("append turn started");
        let uri = format!("/v1/threads/{}/export", created.thread_id);

        let first = get_json(&app, &uri).await;
        assert_eq!(first.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();

        let second = get_json(&app, &uri).await;
        assert_eq!(second.status(), StatusCode::OK);
        let bytes2 = axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            bytes, bytes2,
            "identical store state must export byte-identically"
        );

        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["format_version"], 1);
        assert_eq!(body["thread_id"], created.thread_id.to_string());
        assert_eq!(body["title"], "export-me");
        let events = body["events"].as_array().expect("events array");
        assert!(!events.is_empty());
        let seqs: Vec<u64> = events
            .iter()
            .map(|event| event["seq"].as_u64().unwrap_or_default())
            .collect();
        let mut sorted = seqs.clone();
        sorted.sort_unstable();
        assert_eq!(seqs, sorted, "events must be in ascending seq order");
    }

    #[tokio::test]
    async fn export_answers_structured_not_found_for_unknown_threads() {
        let dir = TempDir::new();
        let (app, _store) = app(&dir);
        let missing = ark_common::ThreadId::new();
        let response = get_json(&app, &format!("/v1/threads/{missing}/export")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
