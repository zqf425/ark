//! HTTP handlers for durable user-managed long-term memory.

use ark_contract::{
    CreateMemoryRequest, MemoryDiagnosticsResponse, MemoryEntry, MemoryListResponse,
    UpdateMemoryRequest,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::api::error::ApiError;
use crate::state::AppState;

fn service(state: &AppState) -> Result<&std::sync::Arc<dyn ark_ports::MemoryService>, ApiError> {
    state.base.memory.as_ref().ok_or_else(|| {
        ApiError::new(
            ark_common::ArkErrorCode::RuntimeUnavailable,
            "memory service is not available in this build",
        )
    })
}

/// `GET /v1/memory`.
pub async fn list(State(state): State<AppState>) -> Result<Json<MemoryListResponse>, ApiError> {
    Ok(Json(service(&state)?.list()?))
}

/// `POST /v1/memory`.
pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateMemoryRequest>,
) -> Result<(StatusCode, Json<MemoryEntry>), ApiError> {
    Ok((StatusCode::CREATED, Json(service(&state)?.create(request)?)))
}

/// `PATCH /v1/memory/{id}`.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateMemoryRequest>,
) -> Result<Json<MemoryEntry>, ApiError> {
    Ok(Json(service(&state)?.update(&id, request)?))
}

/// `DELETE /v1/memory/{id}`.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    service(&state)?.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/memory/diagnostics`.
pub async fn diagnostics(
    State(state): State<AppState>,
) -> Result<Json<MemoryDiagnosticsResponse>, ApiError> {
    Ok(Json(service(&state)?.diagnostics()?))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{BaseServices, MemoryService, SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, MemoryFileStore, StoreConfig};
    use axum::extract::State;

    use super::*;

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
    impl crate::TurnRunner for NoopRunner {
        async fn run_turn(&self, _job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
            Ok(crate::TurnEnd::Completed)
        }
    }

    fn data_dir() -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("ark-memory-http-{stamp}"))
    }

    fn state(data_dir: &std::path::Path, memory: Option<Arc<dyn MemoryService>>) -> AppState {
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"));
        let store: Arc<dyn SessionStore> = sessions;
        let mut base = BaseServices::new();
        if let Some(memory) = memory {
            base = base.with_memory(memory);
        }
        AppState::new(
            store,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(NoopRunner),
            data_dir.to_path_buf(),
            "test",
        )
        .with_base_services(base)
    }

    #[tokio::test]
    async fn all_memory_handlers_support_the_happy_path() {
        let data_dir = data_dir();
        let memory: Arc<dyn MemoryService> =
            Arc::new(MemoryFileStore::open(&data_dir).expect("memory store"));
        let state = state(&data_dir, Some(memory));

        assert!(list(State(state.clone()))
            .await
            .expect("list")
            .0
            .entries
            .is_empty());
        let created = create(
            State(state.clone()),
            Json(CreateMemoryRequest {
                text: "偏好简体中文".to_string(),
                tags: vec![],
                source: None,
            }),
        )
        .await
        .expect("create")
        .1
         .0;
        let updated = update(
            State(state.clone()),
            Path(created.id.clone()),
            Json(UpdateMemoryRequest {
                text: Some("偏好简体中文回复".to_string()),
                tags: None,
                source: None,
            }),
        )
        .await
        .expect("update")
        .0;
        assert_eq!(updated.text, "偏好简体中文回复");
        assert_eq!(
            diagnostics(State(state.clone()))
                .await
                .expect("diagnostics")
                .0
                .entry_count,
            1
        );
        assert_eq!(
            delete(State(state), Path(created.id))
                .await
                .expect("delete"),
            StatusCode::NO_CONTENT
        );

        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn validation_missing_service_and_unknown_ids_are_structured_errors() {
        let data_dir = data_dir();
        let unavailable = list(State(state(&data_dir, None)))
            .await
            .expect_err("unavailable");
        assert_eq!(
            unavailable.0.code,
            ark_common::ArkErrorCode::RuntimeUnavailable
        );

        let memory: Arc<dyn MemoryService> =
            Arc::new(MemoryFileStore::open(&data_dir).expect("memory store"));
        let state = state(&data_dir, Some(memory));
        let invalid = create(
            State(state.clone()),
            Json(CreateMemoryRequest {
                text: " ".to_string(),
                tags: vec![],
                source: None,
            }),
        )
        .await
        .expect_err("empty text");
        assert_eq!(invalid.0.code, ark_common::ArkErrorCode::ValidationError);
        let missing_update = update(
            State(state.clone()),
            Path("mem_missing".to_string()),
            Json(UpdateMemoryRequest::default()),
        )
        .await
        .expect_err("unknown id");
        assert_eq!(missing_update.0.code, ark_common::ArkErrorCode::NotFound);
        let missing = delete(State(state), Path("mem_missing".to_string()))
            .await
            .expect_err("unknown id");
        assert_eq!(missing.0.code, ark_common::ArkErrorCode::NotFound);

        let _ = std::fs::remove_dir_all(data_dir);
    }
}
