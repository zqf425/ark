//! Diagnostics and lifecycle routes (design §7.9, appendix A).

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

use ark_contract::{CapabilitiesResponse, Capability, RuntimeInfo, UiCatalog, UsageResponse};

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
/// Each capability is reported by the port that actually owns it: `tool` by the
/// tool registry, `model_provider` by the provider table, and `ui_page`/
/// `ui_panel` by the UI bus's own registrations. Composing here keeps the
/// composition root from maintaining a parallel grant list that could drift from
/// reality.
///
/// A build without a UI bus reports no pages and no `ui_page` instead of
/// claiming navigation nothing can produce.
pub async fn capabilities(State(state): State<AppState>) -> Json<CapabilitiesResponse> {
    let mut capabilities = Vec::new();

    if !state.tools.names().is_empty() {
        capabilities.push(Capability::Tool);
    }
    if state
        .base
        .providers
        .as_ref()
        .is_some_and(|providers| !providers.list().providers.is_empty())
    {
        capabilities.push(Capability::ModelProvider);
    }

    let catalog = match state.base.ui.as_ref() {
        Some(ui) => {
            capabilities.extend(ui.capabilities());
            ui.catalog(&capabilities)
        }
        None => UiCatalog::default(),
    };
    capabilities.sort();
    capabilities.dedup();

    Json(CapabilitiesResponse {
        version: state.config.version(),
        capabilities,
        catalog,
    })
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

        fn catalog(&self, _available: &[ark_contract::Capability]) -> ark_contract::UiCatalog {
            ark_contract::UiCatalog::default()
        }
    }

    /// Removes its directory on drop, so a failing assertion cannot leak the
    /// store the state opened.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ark-caps-test-{}-{}",
                std::process::id(),
                chrono::Utc::now()
                    .timestamp_nanos_opt()
                    .expect("system clock")
            ));
            std::fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn state_with_ui(dir: &TempDir, ui: Option<Arc<dyn ark_ports::UiCapability>>) -> AppState {
        let store = Arc::new(
            ark_store::FileSessionStore::open(ark_store::StoreConfig::new(&dir.0))
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
            dir.0.clone(),
            "test",
        )
        .with_base_services(base)
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
    async fn capabilities_report_ui_capabilities_from_the_bus() {
        let dir = TempDir::new();
        let state = state_with_ui(
            &dir,
            Some(Arc::new(StubUi {
                caps: vec![
                    ark_contract::Capability::UiPage,
                    ark_contract::Capability::UiPanel,
                ],
            })),
        );
        let Json(body) = capabilities(State(state)).await;
        assert!(body.capabilities.contains(&Capability::UiPage));
        assert!(body.capabilities.contains(&Capability::UiPanel));
        // The test state has no tools and no providers, so those capabilities
        // must not be claimed just because a bus is present.
        assert!(!body.capabilities.contains(&Capability::Tool));
        assert!(!body.capabilities.contains(&Capability::ModelProvider));
        assert!(body.catalog.pages.is_empty());
    }

    #[tokio::test]
    async fn capabilities_do_not_claim_a_ui_without_a_bus() {
        // A build that wires no bus must not advertise `ui_page`: the renderer
        // would then expect navigation that nothing can produce.
        let dir = TempDir::new();
        let state = state_with_ui(&dir, None);
        let Json(body) = capabilities(State(state)).await;
        assert!(body.capabilities.is_empty());
        assert!(body.catalog.pages.is_empty());
        assert!(body.catalog.panels.is_empty());
    }
}
