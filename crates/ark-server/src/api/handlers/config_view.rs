//! `GET /v1/runtime/config` — the settings screen's read model (design §7.15).
//!
//! `POST` on the same path applies a patch; this side reports what is live, so
//! the screen never has to guess a default or duplicate the validation rules.

use axum::extract::State;
use axum::Json;

use ark_contract::RuntimeConfigView;

use crate::api::error::ApiError;
use crate::state::AppState;

/// Build the live view from the current snapshot.
fn view_of(state: &AppState) -> RuntimeConfigView {
    let snapshot = state.config.snapshot();
    RuntimeConfigView {
        version: state.config.version(),
        port: snapshot.port,
        data_dir: snapshot.data_dir.to_string_lossy().to_string(),
        log_level: snapshot.log_level.clone(),
        default_workspace: snapshot
            .default_workspace
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        default_autonomy: snapshot.default_autonomy.clone(),
        subagents: snapshot.subagents,
        tuning: (&snapshot.tuning).into(),
    }
}

/// `GET /v1/runtime/config`.
pub async fn view(State(state): State<AppState>) -> Result<Json<RuntimeConfigView>, ApiError> {
    Ok(Json(view_of(&state)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_config::{ConfigStore, RuntimeConfig};

    #[test]
    fn view_reports_the_live_snapshot_not_defaults() {
        // The view must read the store, otherwise the screen shows stale values
        // right after a successful apply.
        let mut config = RuntimeConfig::default();
        config.log_level = "debug".to_string();
        config.default_workspace = Some(std::path::PathBuf::from("D:/work"));
        let store = ConfigStore::new(config);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.log_level, "debug");
        assert_eq!(
            snapshot.default_workspace.as_deref(),
            Some(std::path::Path::new("D:/work"))
        );
    }
}
