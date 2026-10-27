//! Error mapping for the HTTP boundary (design §6.2 / §7.9).
//!
//! Every failure — including "route not implemented yet" — is an
//! [`ArkErrorPayload`] with a code whose `http_status()` decides the response
//! status. Routes therefore never hand-write a status code or an error body.

use ark_common::{ArkError, ArkErrorCode, ArkErrorPayload};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// Newtype so `?` works in handlers whose success type is a tuple.
#[derive(Debug)]
pub struct ApiError(pub ArkError);

impl ApiError {
    pub fn new(code: ArkErrorCode, message: impl Into<String>) -> Self {
        Self(ArkError::new(code, message))
    }

    /// A route that exists in the old runtime but not in this milestone.
    ///
    /// Structured `NotFound` on purpose: the renderer gets a machine-readable
    /// answer instead of a panic or a 500 (design appendix A).
    pub fn not_implemented(route: &str, milestone: &str) -> Self {
        Self(
            ArkError::not_found(format!("{route} is not implemented yet"))
                .with_detail(format!("planned milestone: {milestone}")),
        )
    }

    pub fn payload(&self) -> ArkErrorPayload {
        self.0.payload()
    }
}

impl From<ArkError> for ApiError {
    fn from(err: ArkError) -> Self {
        Self(err)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.code.http_status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.payload())).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_comes_from_the_error_code() {
        let error = ApiError::new(ArkErrorCode::Conflict, "duplicate");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn unimplemented_routes_are_structured_not_found() {
        let error = ApiError::not_implemented("GET /v1/memory", "M4");
        assert_eq!(error.0.code, ArkErrorCode::NotFound);
        let payload = error.payload();
        assert!(payload.message.contains("GET /v1/memory"));
        assert!(payload.detail.as_deref().unwrap_or_default().contains("M4"));
    }

    #[test]
    fn ark_errors_convert_without_losing_the_code() {
        let error: ApiError = ArkError::validation("bad input").into();
        assert_eq!(error.0.code, ArkErrorCode::ValidationError);
    }
}
