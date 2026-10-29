//! Routes from design appendix A that have not landed yet.
//!
//! They answer a structured `NotFound` (`ArkErrorPayload`) naming the planned
//! milestone. This is deliberate: the surface stays discoverable, no route
//! panics, and a client can distinguish "not built yet" from "wrong URL".

use axum::Json;

use crate::api::error::ApiError;

/// Milestone labels used in the `detail` field.
pub const M_FORK: &str = "M2 thread forking";
pub const M_PLAN: &str = "M3 goal and todo planning";
pub const M_GATE: &str = "M3 approvals and user inputs";
pub const M_ATTACH: &str = "M4 attachment store";
pub const M_CHILD: &str = "M5 child runs";
pub const M_SKILLS: &str = "M5 skills";
pub const M_CONFIG: &str = "M4 split config endpoints folded into POST /v1/runtime/config";
pub const M_DEBUG: &str = "M5 llm round diagnostics";
pub const M_WORKSPACE: &str = "M2 workspace status";

/// `POST /v1/threads/{id}/fork`.
pub async fn fork() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "POST /v1/threads/{id}/fork",
        M_FORK,
    ))
}

/// `GET|POST|DELETE /v1/threads/{id}/goal`.
pub async fn goal() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "GET|POST|DELETE /v1/threads/{id}/goal",
        M_PLAN,
    ))
}

/// `GET|POST|DELETE /v1/threads/{id}/todos`.
pub async fn todos() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "GET|POST|DELETE /v1/threads/{id}/todos",
        M_PLAN,
    ))
}

/// `POST /v1/approvals/{id}` and `POST /v1/user-inputs/{id}`.
pub async fn approvals() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented("POST /v1/approvals/{id}", M_GATE))
}

/// `POST /v1/user-inputs/{id}` (alias `/v1/user-input/{id}`).
pub async fn user_inputs() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "POST /v1/user-inputs/{id}",
        M_GATE,
    ))
}

/// `POST /v1/attachments` and the read routes.
pub async fn attachments() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented("POST /v1/attachments", M_ATTACH))
}

/// `GET /v1/threads/{id}/skills` and `GET /v1/skills`.
pub async fn skills() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented("GET /v1/skills", M_SKILLS))
}

/// The `child-runs` family.
pub async fn child_runs() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented("GET /v1/child-runs", M_CHILD))
}

/// `POST /v1/sessions/{id}/resume-thread`.
pub async fn resume_thread() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "POST /v1/sessions/{id}/resume-thread",
        M_FORK,
    ))
}

/// `POST /v1/threads/{id}/rewind`.
pub async fn rewind() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "POST /v1/threads/{id}/rewind",
        M_FORK,
    ))
}

/// `POST /v1/threads/{id}/review`.
pub async fn review() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "POST /v1/threads/{id}/review",
        M_PLAN,
    ))
}

/// The old split config endpoints: folded into `POST /v1/runtime/config`.
pub async fn model_providers() -> Result<Json<serde_json::Value>, ApiError> {
    Err(
        ApiError::not_implemented("POST /v1/runtime/model-providers", M_CONFIG)
            .with_hint("use POST /v1/runtime/config with a ConfigPatch instead"),
    )
}

/// `GET /v1/debug/llm-rounds`.
pub async fn llm_rounds() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "GET /v1/debug/llm-rounds",
        M_DEBUG,
    ))
}

/// `GET /v1/workspace/status`.
pub async fn workspace_status() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "GET /v1/workspace/status",
        M_WORKSPACE,
    ))
}

impl ApiError {
    /// Add a migration hint to the error detail.
    pub fn with_hint(mut self, hint: &str) -> Self {
        let detail = self
            .0
            .detail
            .take()
            .map(|existing| format!("{existing}; {hint}"))
            .unwrap_or_else(|| hint.to_string());
        self.0.detail = Some(detail);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::ArkErrorCode;

    #[tokio::test]
    async fn pending_routes_are_structured_not_found() {
        let error = fork().await.unwrap_err();
        assert_eq!(error.0.code, ArkErrorCode::NotFound);
        assert!(error.0.detail.unwrap_or_default().contains(M_FORK));
    }

    #[tokio::test]
    async fn split_config_routes_point_at_the_single_entry() {
        let error = model_providers().await.unwrap_err();
        let detail = error.0.detail.unwrap_or_default();
        assert!(detail.contains("POST /v1/runtime/config"));
    }

    #[tokio::test]
    async fn every_pending_route_returns_not_found() {
        let cases = [
            goal().await.err(),
            todos().await.err(),
            approvals().await.err(),
            user_inputs().await.err(),
            attachments().await.err(),
            skills().await.err(),
            child_runs().await.err(),
            resume_thread().await.err(),
            rewind().await.err(),
            review().await.err(),
            llm_rounds().await.err(),
            workspace_status().await.err(),
        ];
        for case in cases {
            let error = case.expect("pending route must fail");
            assert_eq!(error.0.code, ArkErrorCode::NotFound);
        }
    }
}
