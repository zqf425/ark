//! Diagnostics and lifecycle routes (design §7.9, appendix A).

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

use ark_contract::{RuntimeInfo, UsageResponse};

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
///
/// Capabilities and the page/panel catalog both come from the injected UI bus,
/// so this route cannot drift from the navigation the renderer actually gets.
/// A build without a bus reports no pages instead of claiming `ui_page`.
pub async fn capabilities(State(state): State<AppState>) -> Json<serde_json::Value> {
    let (capabilities, catalog) = match state.base.ui.as_ref() {
        Some(ui) => (ui.capabilities(), ui.catalog_json()),
        None => (Vec::new(), json!({ "pages": [], "panels": [] })),
    };
    Json(json!({
        "version": state.config.version(),
        "capabilities": capabilities,
        "catalog": catalog,
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

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ports::BaseServices;
    use std::sync::Arc;

    use crate::state::AppState;

    /// Minimal bus: the route must not care how the catalog was produced.
    struct StubUi {
        caps: Vec<ark_contract::Capability>,
    }

    impl ark_ports::UiCapability for StubUi {
        fn register_page(
            &self,
            page: ark_contract::PageSpec,
        ) -> ark_common::Result<ark_ports::PageHandle> {
            Ok(ark_ports::PageHandle { id: page.id.0 })
        }

        fn register_panel(
            &self,
            _slot: ark_contract::PanelSlot,
            panel: ark_contract::PanelSpec,
        ) -> ark_common::Result<ark_ports::PanelHandle> {
            Ok(ark_ports::PanelHandle { id: panel.id.0 })
        }

        fn pages(&self) -> Vec<ark_contract::PageSpec> {
            Vec::new()
        }

        fn capabilities(&self) -> Vec<ark_contract::Capability> {
            self.caps.clone()
        }

        fn catalog_json(&self) -> serde_json::Value {
            json!({ "pages": [], "panels": [] })
        }
    }

    fn state_with_ui(ui: Option<Arc<dyn ark_ports::UiCapability>>) -> AppState {
        let dir = std::env::temp_dir().join(format!("ark-caps-{}", uuid_like()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let store = Arc::new(
            ark_store::FileSessionStore::open(ark_store::StoreConfig::new(&dir))
                .expect("open store"),
        );
        let base = match ui {
            Some(ui) => BaseServices::new().with_ui(ui),
            None => BaseServices::new(),
        };
        AppState::new(
            store,
            Arc::new(EmptyTools),
            Arc::new(ark_config::ConfigStore::new(
                ark_config::RuntimeConfig::default(),
            )),
            Arc::new(ark_obs::UsageRecorder::new()),
            Arc::new(PendingRunner),
            dir,
            "test",
        )
        .with_base_services(base)
    }

    fn uuid_like() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    }

    struct EmptyTools;

    impl ark_ports::ToolRegistryPort for EmptyTools {
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

    struct PendingRunner;

    #[async_trait::async_trait]
    impl crate::state::TurnRunner for PendingRunner {
        async fn run_turn(
            &self,
            _job: crate::state::TurnJob,
        ) -> ark_common::Result<crate::state::TurnEnd> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn capabilities_come_from_the_injected_bus() {
        let state = state_with_ui(Some(Arc::new(StubUi {
            caps: vec![
                ark_contract::Capability::Tool,
                ark_contract::Capability::UiPage,
            ],
        })));
        let Json(body) = capabilities(State(state)).await;
        let caps = body["capabilities"].as_array().expect("capabilities array");
        assert_eq!(caps.len(), 2);
        assert!(caps.iter().any(|cap| cap == "tool"));
        assert!(caps.iter().any(|cap| cap == "ui_page"));
        assert!(body["catalog"]["pages"].is_array());
    }

    #[tokio::test]
    async fn capabilities_do_not_claim_a_ui_without_a_bus() {
        // A build that wires no bus must not advertise `ui_page`: the renderer
        // would then expect navigation that nothing can produce.
        let state = state_with_ui(None);
        let Json(body) = capabilities(State(state)).await;
        let caps = body["capabilities"].as_array().expect("capabilities array");
        assert!(caps.is_empty());
        assert_eq!(body["catalog"]["pages"].as_array().map(Vec::len), Some(0));
    }
}
