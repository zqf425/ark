//! `POST /v1/runtime/config` — the single config entry point (design §7.15).
//!
//! This route replaces the four old endpoints. Validation, the version bump and
//! the `Restart` rejection all happen inside `ConfigStore::apply`, so the handler
//! only has to report the result and persist a successful snapshot.

use axum::extract::State;
use axum::Json;

use ark_contract::{ApplyResult, ConfigPatch};

use crate::api::error::ApiError;
use crate::state::AppState;

/// Apply one `ConfigPatch`.
///
/// A rejected patch answers `200` with `result: rejected`: the request was
/// understood, it is the *content* that cannot take effect without a restart,
/// and the body names the fields. That keeps the client's error handling simple
/// and matches the `ApplyResult` contract.
pub async fn apply(
    State(state): State<AppState>,
    Json(patch): Json<ConfigPatch>,
) -> Result<Json<ApplyResult>, ApiError> {
    let before = state.config.snapshot().subagents;
    let result = state.config.apply(patch);
    if let ApplyResult::Applied { .. } = result {
        // Persist the accepted snapshot so a restart keeps it. A write failure is
        // reported but does not roll back the live config: the running process
        // is already on the new version and only durability is at risk.
        let snapshot = state.config.snapshot();
        let path = ark_config::config_path(&state.data_dir);
        if let Err(message) = ark_config::save(&path, &snapshot) {
            tracing::warn!(%message, "config applied in memory but not persisted");
        }
        let after = snapshot.subagents;
        if before.enabled != after.enabled || before.auto_continue != after.auto_continue {
            crate::service::continuation::sync_auto_continue_policy(state.clone()).await;
        }
    }
    Ok(Json(result))
}
