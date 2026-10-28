//! Human-in-the-loop gate resolution routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use ark_contract::{ResolveApprovalRequest, ResolveUserInputRequest};

use crate::api::error::ApiError;
use crate::state::AppState;

/// `POST /v1/approvals/{id}`.
pub async fn resolve_approval(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ResolveApprovalRequest>,
) -> Result<StatusCode, ApiError> {
    if state.gates.resolve_approval(&id, body.approved) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            ark_common::ArkErrorCode::NotFound,
            "approval is not pending",
        ))
    }
}

/// `POST /v1/user-inputs/{id}` and the legacy singular alias.
pub async fn resolve_user_input(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ResolveUserInputRequest>,
) -> Result<StatusCode, ApiError> {
    let answer = body.answer.trim();
    if answer.is_empty() {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            "user input answer must not be empty",
        ));
    }
    if state.gates.resolve_user_input(&id, answer.to_string()) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            ark_common::ArkErrorCode::NotFound,
            "user input request is not pending",
        ))
    }
}
