//! `POST /v1/subagents/config` updates only the sub-agent policy.

use axum::extract::State;
use axum::Json;

use ark_contract::{ApplyResult, ConfigPatch, SubagentSettings};

use crate::api::error::ApiError;
use crate::state::AppState;

/// Return the live sub-agent switch and concurrency cap.
pub async fn view(State(state): State<AppState>) -> Result<Json<SubagentSettings>, ApiError> {
    Ok(Json(state.config.snapshot().subagents))
}

/// Apply the hot sub-agent switch and concurrency cap, returning its live view.
pub async fn configure(
    State(state): State<AppState>,
    Json(settings): Json<SubagentSettings>,
) -> Result<Json<ApplyResult>, ApiError> {
    let before = state.config.snapshot().subagents;
    let path = ark_config::config_path(&state.config_dir);
    let result = state.config.apply_with(
        ConfigPatch {
            subagents: Some(settings),
            ..ConfigPatch::default()
        },
        |snapshot| ark_config::save(&path, snapshot),
    );
    if matches!(result, ark_contract::ApplyResult::Applied { .. }) {
        let snapshot = state.config.snapshot();
        let after = snapshot.subagents;
        if before.enabled != after.enabled || before.auto_continue != after.auto_continue {
            crate::service::continuation::sync_auto_continue_policy(state.clone()).await;
        }
    }
    Ok(Json(result))
}
