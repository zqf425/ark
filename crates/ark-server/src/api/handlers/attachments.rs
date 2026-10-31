//! HTTP handlers for immutable user attachments.

use std::str::FromStr;

use ark_common::{ArkErrorCode, AttachmentId};
use ark_contract::{AttachmentUploadRequest, AttachmentUploadResponse};
use ark_ports::AttachmentUpload;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use base64::Engine;

use crate::api::error::ApiError;
use crate::state::AppState;

/// `POST /v1/attachments` with a Base64 JSON body.
pub async fn upload(
    State(state): State<AppState>,
    request: Result<Json<AttachmentUploadRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<AttachmentUploadResponse>), ApiError> {
    let store = attachment_store(&state)?;
    let Json(request) = request.map_err(|rejection| match rejection {
        JsonRejection::BytesRejection(_) => upload_size_error(store.max_bytes()),
        _ => ApiError::new(
            ArkErrorCode::ValidationError,
            "invalid attachment upload request",
        ),
    })?;
    let max_encoded = encoded_limit(store.max_bytes());
    if request.content.len() > max_encoded {
        return Err(upload_size_error(store.max_bytes()));
    }
    let content = base64::engine::general_purpose::STANDARD
        .decode(request.content)
        .map_err(|_| {
            ApiError::new(
                ArkErrorCode::ValidationError,
                "attachment content must be valid Base64",
            )
        })?;
    let attachment = store.upload(AttachmentUpload {
        original_filename: request.original_filename,
        mime_type: request.mime_type,
        preview_text: request.preview_text,
        content,
    })?;
    Ok((
        StatusCode::CREATED,
        Json(AttachmentUploadResponse { attachment }),
    ))
}

/// `GET /v1/attachments/{id}`.
pub async fn get(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<Json<ark_contract::AttachmentEntry>, ApiError> {
    let entry = attachment_store(&state)?.get(parse_id(&raw_id)?)?;
    Ok(Json(entry))
}

/// `GET /v1/attachments/{id}/content`.
pub async fn content(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<Response<Body>, ApiError> {
    let store = attachment_store(&state)?;
    let entry = store.get(parse_id(&raw_id)?)?;
    let bytes = store.read(entry.id)?;
    let content_type = entry
        .mime_type
        .as_deref()
        .and_then(|value| HeaderValue::from_str(value).ok())
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
    let length = HeaderValue::from_str(&bytes.len().to_string())
        .map_err(|_| ApiError::new(ArkErrorCode::InternalError, "invalid attachment length"))?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_LENGTH, length),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (
                header::CONTENT_DISPOSITION,
                content_disposition(&entry.original_filename),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// `GET /v1/attachments/diagnostics`.
pub async fn diagnostics(
    State(state): State<AppState>,
) -> Result<Json<ark_contract::AttachmentDiagnostics>, ApiError> {
    Ok(Json(attachment_store(&state)?.diagnostics()?))
}

fn attachment_store(
    state: &AppState,
) -> Result<&std::sync::Arc<dyn ark_ports::AttachmentStore>, ApiError> {
    state.attachments.as_ref().ok_or_else(|| {
        ApiError::new(
            ArkErrorCode::RuntimeUnavailable,
            "attachment storage is unavailable",
        )
    })
}

fn parse_id(raw: &str) -> Result<AttachmentId, ApiError> {
    AttachmentId::from_str(raw)
        .map_err(|_| ApiError::new(ArkErrorCode::NotFound, "attachment does not exist"))
}

fn encoded_limit(max_bytes: u64) -> usize {
    max_bytes
        .saturating_add(2)
        .saturating_div(3)
        .saturating_mul(4) as usize
}

fn upload_size_error(max_bytes: u64) -> ApiError {
    ApiError::new(
        ArkErrorCode::ValidationError,
        format!("attachment content exceeds the {max_bytes} byte limit"),
    )
}

fn content_disposition(original_filename: &str) -> HeaderValue {
    let mut filename: String = original_filename
        .chars()
        .take(128)
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => character,
            _ => '_',
        })
        .collect();
    if filename.is_empty() {
        filename.push_str("attachment");
    }
    HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .expect("sanitized attachment filename is a valid header value")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ark_common::Result;
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::ToolRegistryPort;
    use ark_store::{AttachmentStore, FileSessionStore, StoreConfig};
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;
    use crate::router;
    use crate::state::{TurnEnd, TurnJob, TurnRunner};

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

    struct NeverRunner;

    #[async_trait::async_trait]
    impl TurnRunner for NeverRunner {
        async fn run_turn(&self, _job: TurnJob) -> Result<TurnEnd> {
            Ok(TurnEnd::Completed)
        }
    }

    fn test_state(data_dir: &std::path::Path) -> AppState {
        test_state_with_attachment_limit(data_dir, 32)
    }

    fn test_state_with_attachment_limit(data_dir: &std::path::Path, max_bytes: u64) -> AppState {
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("sessions open"));
        let attachments =
            Arc::new(AttachmentStore::open(data_dir, max_bytes).expect("attachments open"));
        AppState::new(
            sessions,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(NeverRunner),
            data_dir.to_path_buf(),
            "test",
        )
        .with_attachments(attachments)
    }

    /// A fresh random data directory, removed when the guard drops.
    ///
    /// `tempfile` names it uniquely, so parallel tests never share a store
    /// directory (and never contend for the same data lock).
    fn test_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("ark-server-attachment-")
            .tempdir()
            .expect("temporary directory is creatable")
    }

    #[tokio::test]
    async fn attachment_routes_preserve_binary_content_and_return_not_found_for_invalid_id() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let app = router(test_state(&data_dir));
        let body = serde_json::json!({
            "original_filename": "pixel.bin",
            "mime_type": "application/octet-stream",
            "content": "AP+A"
        });
        let upload = app
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
        assert_eq!(upload.status(), StatusCode::CREATED);
        let uploaded: serde_json::Value = serde_json::from_slice(
            &to_bytes(upload.into_body(), usize::MAX)
                .await
                .expect("upload body reads"),
        )
        .expect("upload response is json");
        let id = uploaded["attachment"]["id"].as_str().expect("id exists");

        let metadata = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/attachments/{id}"))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("route responds");
        assert_eq!(metadata.status(), StatusCode::OK);

        let content = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/attachments/{id}/content"))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("route responds");
        assert_eq!(content.status(), StatusCode::OK);
        assert_eq!(
            content.headers()["content-type"],
            "application/octet-stream"
        );
        assert_eq!(content.headers()["content-length"], "3");
        assert_eq!(
            to_bytes(content.into_body(), usize::MAX)
                .await
                .expect("content reads"),
            vec![0, 255, 128]
        );

        let diagnostics = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/attachments/diagnostics")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("route responds");
        assert_eq!(diagnostics.status(), StatusCode::OK);

        let invalid = app
            .oneshot(
                Request::builder()
                    .uri("/v1/attachments/not-a-uuid")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("route responds");
        assert_eq!(invalid.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn attachment_uploads_larger_than_axums_default_body_limit() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let app = router(test_state_with_attachment_limit(&data_dir, 4 * 1024 * 1024));
        let body = serde_json::json!({
            "original_filename": "large.bin",
            "mime_type": "application/octet-stream",
            "content": base64::engine::general_purpose::STANDARD
                .encode(vec![0_u8; 4 * 1024 * 1024 - 1024]),
        });

        let response = app
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
    }

    #[tokio::test]
    async fn oversized_attachment_upload_returns_a_structured_error() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let app = router(test_state_with_attachment_limit(&data_dir, 32));
        let body = serde_json::json!({
            "original_filename": "too-large.bin",
            "content": base64::engine::general_purpose::STANDARD.encode(vec![0_u8; 33]),
        });

        let response = app
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

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let payload: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response body reads"),
        )
        .expect("response is json");
        assert_eq!(payload["code"], "validation_error");
        assert!(payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("32"));
    }

    #[tokio::test]
    async fn upload_body_limit_returns_a_structured_error() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let app = router(test_state_with_attachment_limit(&data_dir, 32));
        let body = serde_json::json!({
            "original_filename": "large-metadata.bin",
            "content": "AA==",
            "preview_text": "x".repeat(70 * 1024),
        });

        let response = app
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

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let payload: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response body reads"),
        )
        .expect("response is json");
        assert_eq!(payload["code"], "validation_error");
        assert!(payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("32"));
    }

    #[tokio::test]
    async fn attachment_content_is_downloaded_with_nosniff_protection() {
        let dir = test_dir();
        let data_dir = dir.path().to_path_buf();
        let app = router(test_state(&data_dir));
        let body = serde_json::json!({
            "original_filename": "unsafe\".html",
            "mime_type": "text/html",
            "content": base64::engine::general_purpose::STANDARD.encode("<script>alert(1)</script>"),
        });
        let upload = app
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
        let uploaded: serde_json::Value = serde_json::from_slice(
            &to_bytes(upload.into_body(), usize::MAX)
                .await
                .expect("upload body reads"),
        )
        .expect("upload response is json");
        let id = uploaded["attachment"]["id"].as_str().expect("id exists");

        let content = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/attachments/{id}/content"))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("route responds");

        assert_eq!(content.status(), StatusCode::OK);
        assert_eq!(content.headers()["content-type"], "text/html");
        assert_eq!(content.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            content.headers()["content-disposition"],
            "attachment; filename=\"unsafe_.html\""
        );
    }
}
