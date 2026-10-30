//! Diagnostics and lifecycle routes (design §7.9, appendix A).

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

use ark_contract::{Capability, RuntimeInfo, UsageResponse};

use crate::api::error::ApiError;
use crate::state::AppState;

/// `GET /v1/runtime/info`.
pub async fn info(State(state): State<AppState>) -> Result<Json<RuntimeInfo>, ApiError> {
    let threads = state.store.list_threads().await?;
    Ok(Json(RuntimeInfo {
        runtime: "arkd".to_string(),
        version: state.version.clone(),
        data_dir: state.data_dir.to_string_lossy().to_string(),
        thread_count: threads.len() as u64,
        started_at: state.started_at,
    }))
}

/// `GET /v1/runtime/tools`.
///
/// The catalogue comes from the injected registry port, so this route cannot
/// drift from what the model actually receives.
pub async fn tools(State(state): State<AppState>) -> Json<serde_json::Value> {
    let names = state.tools.names();
    let schemas = state.tools.schemas();
    let entries: Vec<serde_json::Value> = schemas
        .iter()
        .map(|schema| {
            json!({
                "name": schema.get("name").and_then(|value| value.as_str()).unwrap_or_default(),
                "schema": schema,
            })
        })
        .collect();
    Json(json!({
        "version": state.tools.version(),
        "names": names,
        "tools": entries,
    }))
}

/// `GET /v1/runtime/capabilities`.
pub async fn capabilities(State(state): State<AppState>) -> Json<serde_json::Value> {
    // Every capability this milestone actually provides. Listing an unimplemented
    // one here would make the route lie, so the set is explicit.
    let capabilities = [
        Capability::Tool,
        Capability::ModelProvider,
        Capability::UiPage,
    ];
    Json(json!({
        "version": state.config.version(),
        "capabilities": capabilities,
    }))
}

/// `GET /v1/usage`.
///
/// Only verified counters are returned: turns completed and provider-reported
/// usage. There is no derived cache hit rate when the provider sent none.
pub async fn usage(State(state): State<AppState>) -> Json<UsageResponse> {
    Json(state.usage_response())
}

/// `POST /v1/shutdown`.
///
/// Cancels every in-flight turn (they land on disk through the reactor's
/// terminal event) and signals the listener to stop.
pub async fn shutdown(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    // Mark shutdown before tripping turns so late completion wakeups are rejected,
    // and persist a process-owned reason rather than user-interrupt intent.
    let notified = state.request_shutdown("http");
    state
        .turns
        .cancel_all_with_reason(crate::state::SHUTDOWN_CANCEL_REASON);
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "shutting_down",
            "listeners_notified": notified,
            "turns_aborted": state.turns.active(),
        })),
    )
}
