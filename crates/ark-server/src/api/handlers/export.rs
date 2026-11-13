//! `GET /v1/threads/{id}/export` - a versioned, deterministic thread snapshot.
//!
//! The export is built only from the durable store: summary metadata plus the
//! full authoritative event log in ascending `seq` order. It adds no export
//! timestamp (that would break determinism), carries no credentials, API keys,
//! headers, or attachment payloads, and replays byte-identically for identical
//! store state.

use ark_common::ArkErrorCode;
use ark_contract::{ThreadExport, THREAD_EXPORT_FORMAT_VERSION};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::handlers::export_zip;
use crate::api::handlers::threads::parse_thread;
use crate::state::AppState;

/// Query parameters of `GET /v1/threads/{id}/export`.
#[derive(Debug, Default, Deserialize)]
pub struct ExportQuery {
    /// `zip` wraps the identical snapshot in a deterministic single-entry
    /// archive; every other value (and no value) keeps the bare JSON body.
    pub format: Option<String>,
}

/// `GET /v1/threads/{id}/export`.
pub async fn export(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let thread_id = parse_thread(&id)?;
    let meta = state.store.get_thread(thread_id).await?;
    let events = state.store.replay_events(thread_id, 0).await?;
    let export = ThreadExport {
        format_version: THREAD_EXPORT_FORMAT_VERSION,
        thread_id: meta.thread_id,
        title: meta.title,
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        archived: meta.archived,
        runtime: meta.runtime,
        events,
    };
    if query.format.as_deref() != Some("zip") {
        return Ok(Json(export).into_response());
    }

    // The zip entry holds exactly the same serialization the JSON route
    // would return, so both formats stay byte-identical for identical store
    // state. The entry name uses the immutable thread id, never the title.
    let json = serde_json::to_vec(&export).map_err(|error| {
        ApiError::new(
            ArkErrorCode::InternalError,
            format!("serialize thread export: {error}"),
        )
    })?;
    let entry_name = format!("thread-{}.json", export.thread_id);
    let zip = export_zip::zip_single_entry(&entry_name, &json);
    let filename = format!("thread-{}.zip", export.thread_id);
    let disposition = format!("attachment; filename=\"{filename}\"");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from(zip))
        .map_err(|error| {
            ApiError::new(
                ArkErrorCode::InternalError,
                format!("build zip export response: {error}"),
            )
        })
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
    use chrono::Utc;
    use tower::ServiceExt;

    use crate::api::routes::router;
    use crate::state::{AppState, TurnEnd, TurnJob, TurnRunner};

    /// Backed by `tempfile`, whose random name keeps two parallel tests from
    /// picking the same path.
    struct TempDir(tempfile::TempDir);

    impl TempDir {
        fn new() -> Self {
            let dir = tempfile::Builder::new()
                .prefix("ark-export-test-")
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
                    attachment_ids: Vec::new(),
                    attachment_outcomes: Vec::new(),
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

    #[tokio::test]
    async fn export_zip_wraps_the_snapshot_deterministically() {
        let dir = TempDir::new();
        let (app, store) = app(&dir);
        let created = store
            .create_thread("zip-me")
            .await
            .expect("create thread");
        let uri = format!("/v1/threads/{}/export?format=zip", created.thread_id);

        let first = get_json(&app, &uri).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(
            first.headers()[axum::http::header::CONTENT_TYPE],
            "application/zip"
        );
        let bytes = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04", "zip local header magic");

        let (name, contents) =
            super::export_zip::read_single_entry(&bytes).expect("single zip entry");
        assert_eq!(name, format!("thread-{}.json", created.thread_id));
        let body: serde_json::Value = serde_json::from_slice(&contents).unwrap();
        assert_eq!(body["title"], "zip-me");
        assert_eq!(body["thread_id"], created.thread_id.to_string());

        let second = get_json(&app, &uri).await;
        let bytes2 = axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, bytes2, "zip export must be byte-identical");
    }
}
