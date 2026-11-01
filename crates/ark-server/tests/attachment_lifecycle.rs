//! End-to-end attachment lifecycle: upload, associate, reference, delete.
//!
//! These live outside the handler module because they are *cross-layer* rules:
//! the authorization edge is a durable event owned by `ark-store`, and the guard
//! that refuses a delete is projected from that log by `ark-server`. A handler
//! unit test could pass while the two sides disagreed about where the authority
//! lives, so every case here goes through the real router and the real stores.

use std::sync::Arc;

use ark_common::Result;
use ark_config::{ConfigStore, RuntimeConfig};
use ark_obs::UsageRecorder;
use ark_ports::ToolRegistryPort;
use ark_server::{router, AppState, TurnEnd, TurnJob, TurnRunner};
use ark_store::{AttachmentStore, FileSessionStore, StoreConfig};
use axum::body::Body;
use axum::http::{Request, StatusCode};
// The trait import is what brings `encode` into scope on the engine value.
use base64::Engine;
use tower::ServiceExt;

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
impl TurnRunner for NoopRunner {
    async fn run_turn(&self, _job: TurnJob) -> Result<TurnEnd> {
        Ok(TurnEnd::Completed)
    }
}

fn temp_dir(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("ark-attachment-lifecycle-{name}-"))
        .tempdir()
        .expect("temporary directory is creatable")
}

fn state(data_dir: &std::path::Path) -> AppState {
    let sessions =
        Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("sessions open"));
    let attachments = Arc::new(AttachmentStore::open(data_dir, 1024).expect("attachments open"));
    AppState::new(
        sessions,
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig::default())),
        Arc::new(UsageRecorder::new()),
        Arc::new(NoopRunner),
        data_dir.to_path_buf(),
        "test",
    )
    .with_attachments(attachments)
}

/// Upload one attachment and return its id.
async fn upload(app: &axum::Router, filename: &str, mime: &str, content: &[u8]) -> String {
    let body = serde_json::json!({
        "original_filename": filename,
        "mime_type": mime,
        "content": base64::engine::general_purpose::STANDARD.encode(content),
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/attachments")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request builds"),
        )
        .await
        .expect("route responds");
    assert_eq!(response.status(), StatusCode::CREATED);
    json_body(response).await["attachment"]["id"]
        .as_str()
        .expect("upload returns an id")
        .to_string()
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    use axum::body::to_bytes;
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads"),
    )
    .expect("response is json")
}

async fn post_json(
    app: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request builds"),
        )
        .await
        .expect("route responds")
}

async fn get(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("route responds")
}

/// A PNG signature followed by extra bytes: enough to satisfy the magic check
/// while staying a distinct payload per test.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nark";

#[tokio::test]
async fn upload_records_the_sha256_of_the_stored_bytes() {
    let dir = temp_dir("sha256");
    let app = router(state(dir.path()));
    let body = serde_json::json!({
        "original_filename": "pixel.png",
        "mime_type": "image/png",
        "content": base64::engine::general_purpose::STANDARD.encode(PNG_BYTES),
    });
    let response = post_json(&app, "/v1/attachments", body).await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let payload = json_body(response).await;
    assert_eq!(
        payload["attachment"]["sha256"],
        "519f9920cf83e4f2c845888bbc35c651c8ef0443e91caa9d9f11411ad88baa2d"
    );
}

#[tokio::test]
async fn declared_mime_that_the_bytes_contradict_is_rejected() {
    let dir = temp_dir("mime");
    let app = router(state(dir.path()));
    let body = serde_json::json!({
        "original_filename": "not-really.png",
        "mime_type": "image/png",
        "content": base64::engine::general_purpose::STANDARD.encode(b"plain text"),
    });

    let response = post_json(&app, "/v1/attachments", body).await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(response).await["code"], "mime_mismatch");
}

#[tokio::test]
async fn association_survives_a_restart_and_shows_up_in_thread_detail() {
    let dir = temp_dir("associate");
    let data_dir = dir.path().to_path_buf();

    // The data directory admits exactly one writer, so each simulated process is
    // fully dropped before the next one opens it. Holding two states at once is
    // not "a restart", it is two daemons on one directory, which the store
    // correctly refuses.
    let (id, thread_id) = {
        let app = router(state(&data_dir));
        let id = upload(&app, "pixel.png", "image/png", PNG_BYTES).await;
        let thread_id = app_owned_thread(&app).await;
        (id, thread_id)
    };

    {
        let app = router(state(&data_dir));
        let response = post_json(
            &app,
            &format!("/v1/threads/{thread_id}/attachments"),
            serde_json::json!({ "attachment_id": id }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        let detail = json_body(get(&app, &format!("/v1/threads/{thread_id}")).await).await;
        assert_eq!(
            detail["attachment_ids"],
            serde_json::json!([id]),
            "thread detail must project the association"
        );
    }

    // Reopening the stores is the restart: the projection is rebuilt from the
    // durable event, so no second association table can silently disagree.
    let app = router(state(&data_dir));
    let reopened = json_body(get(&app, &format!("/v1/threads/{thread_id}")).await).await;
    assert_eq!(reopened["attachment_ids"], serde_json::json!([id]));
}

/// Create a thread through the live store the router uses.
async fn app_owned_thread(app: &axum::Router) -> String {
    let response = post_json(app, "/v1/threads", serde_json::json!({ "title": "t" })).await;
    json_body(response).await["thread_id"]
        .as_str()
        .expect("thread id")
        .to_string()
}

#[tokio::test]
async fn a_turn_may_only_reference_attachments_associated_with_its_own_thread() {
    let dir = temp_dir("turn");
    let data_dir = dir.path().to_path_buf();
    let app = router(state(&data_dir));
    let id = upload(&app, "pixel.png", "image/png", PNG_BYTES).await;
    let thread_id = app_owned_thread(&app).await;

    let response = post_json(
        &app,
        &format!("/v1/threads/{thread_id}/turns"),
        serde_json::json!({ "text": "look at this", "attachment_ids": [id] }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        json_body(response).await["code"],
        "attachment_not_associated"
    );
}

#[tokio::test]
async fn deleting_an_associated_attachment_is_refused() {
    let dir = temp_dir("delete-guard");
    let data_dir = dir.path().to_path_buf();
    let app = router(state(&data_dir));
    let id = upload(&app, "pixel.png", "image/png", PNG_BYTES).await;
    let thread_id = app_owned_thread(&app).await;
    let associated = post_json(
        &app,
        &format!("/v1/threads/{thread_id}/attachments"),
        serde_json::json!({ "attachment_id": id }),
    )
    .await;
    assert_eq!(associated.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/attachments/{id}"))
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("route responds");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "attachment_in_use");
    // The bytes must still be there: a refused delete deletes nothing.
    assert_eq!(
        get(&app, &format!("/v1/attachments/{id}/content"))
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn deleting_an_unassociated_attachment_removes_its_bytes() {
    let dir = temp_dir("delete");
    let data_dir = dir.path().to_path_buf();
    let app = router(state(&data_dir));
    let id = upload(&app, "orphan.png", "image/png", PNG_BYTES).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/attachments/{id}"))
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("route responds");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        get(&app, &format!("/v1/attachments/{id}")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &format!("/v1/attachments/{id}/content"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
