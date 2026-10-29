//! HTTP handlers for immutable user attachments.

use std::str::FromStr;

use ark_common::{ArkError, ArkErrorCode, AttachmentId};
use ark_contract::{AttachmentUploadRequest, AttachmentUploadResponse};
use ark_ports::AttachmentUpload;
use axum::body::Body;
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
    Json(request): Json<AttachmentUploadRequest>,
) -> Result<(StatusCode, Json<AttachmentUploadResponse>), ApiError> {
    let store = attachment_store(&state)?;
    let max_encoded = encoded_limit(store.max_bytes());
    if request.content.len() > max_encoded {
        return Err(
            ArkError::validation("attachment content exceeds the configured size limit").into(),
        );
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
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("sessions open"));
        let attachments = Arc::new(AttachmentStore::open(data_dir, 32).expect("attachments open"));
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

    fn test_dir() -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("ark-server-attachment-{}", AttachmentId::new()));
        std::fs::create_dir_all(&path).expect("temporary directory is creatable");
        path
    }

    #[tokio::test]
    async fn attachment_routes_preserve_binary_content_and_return_not_found_for_invalid_id() {
        let data_dir = test_dir();
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
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
