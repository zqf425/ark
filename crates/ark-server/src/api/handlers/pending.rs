//! Routes that intentionally answer a structured `NotFound`.
//!
//! Every route registered here has a real implementation elsewhere except the
//! three legacy split-config entry points and one maintenance stub, which are
//! kept only so an old client gets an actionable answer instead of a router-level
//! 404. Handlers whose milestone has since landed must be deleted from this
//! module: leaving them behind makes the "what is still missing" question
//! unanswerable from the route table.

use axum::Json;

use crate::api::error::ApiError;

/// Milestone label used in the `detail` field of a retired route.
pub const M_CONFIG: &str = "M4 split config endpoints folded into POST /v1/runtime/config";
/// Milestone label for the child-run maintenance stub.
pub const M_CHILD: &str = "M5 child runs";

/// The old split config endpoints: folded into `POST /v1/runtime/config`.
pub async fn model_providers() -> Result<Json<serde_json::Value>, ApiError> {
    Err(
        ApiError::not_implemented("POST /v1/runtime/model-providers", M_CONFIG)
            .with_hint("use POST /v1/runtime/config with a ConfigPatch instead"),
    )
}

/// Legacy tuning route; clients must use the single config patch entry point.
pub async fn tuning() -> Result<Json<serde_json::Value>, ApiError> {
    Err(
        ApiError::not_implemented("POST /v1/runtime/tuning", M_CONFIG)
            .with_hint("use POST /v1/runtime/config with a ConfigPatch instead"),
    )
}

/// Connector environment storage has not landed in this runtime.
pub async fn connector_env() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "POST /v1/connector-env",
        M_CONFIG,
    ))
}

/// `DELETE /v1/child-runs/{child_id}/cleanup`.
///
/// Startup reconciliation already removes leftover tombstones, so this route
/// exists only so an old client gets a structured answer instead of 404 at the
/// router level.
pub async fn child_runs() -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented(
        "DELETE /v1/child-runs/{child_id}/cleanup",
        M_CHILD,
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
    async fn retired_routes_are_structured_not_found() {
        let cases = [
            model_providers().await.err(),
            tuning().await.err(),
            connector_env().await.err(),
            child_runs().await.err(),
        ];
        for case in cases {
            let error = case.expect("retired route must fail");
            assert_eq!(error.0.code, ArkErrorCode::NotFound);
        }
    }

    #[tokio::test]
    async fn split_config_routes_point_at_the_single_entry() {
        let error = model_providers().await.unwrap_err();
        let detail = error.0.detail.unwrap_or_default();
        assert!(detail.contains("POST /v1/runtime/config"));
    }

    #[tokio::test]
    async fn split_config_errors_name_their_own_routes() {
        let cases = [
            ("POST /v1/runtime/model-providers", model_providers().await),
            ("POST /v1/runtime/tuning", tuning().await),
            ("POST /v1/connector-env", connector_env().await),
        ];
        for (route, result) in cases {
            let error = result.expect_err("retired route must fail");
            assert!(error.payload().message.contains(route));
        }
    }
}
