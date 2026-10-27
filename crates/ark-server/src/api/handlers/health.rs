//! `GET /health` — unauthenticated liveness probe (design §7.9).

use axum::Json;
use chrono::Utc;

use crate::state::AppState;

/// Liveness probe. Mounted outside the auth layer on purpose.
pub async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<ark_contract::HealthResponse> {
    let uptime_ms = (Utc::now() - state.started_at).num_milliseconds().max(0) as u64;
    Json(ark_contract::HealthResponse {
        status: "ok".to_string(),
        version: state.version.clone(),
        uptime_ms,
    })
}
