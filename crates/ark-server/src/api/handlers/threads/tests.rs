use std::sync::Arc;

use super::*;
use ark_common::{ChildId, TurnId};
use ark_config::{ConfigStore, RuntimeConfig};
use ark_obs::UsageRecorder;
use ark_ports::{
    BaseServices, ModelEndpoint, ProviderService, SessionStore, ThreadMeta, ToolRegistryPort,
};
use ark_store::{FileSessionStore, StoreConfig};
use axum::extract::{Path, State};

struct EmptyTools;

impl ToolRegistryPort for EmptyTools {
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

struct NoopRunner;

#[async_trait::async_trait]
impl crate::TurnRunner for NoopRunner {
    async fn run_turn(&self, _job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
        Ok(crate::TurnEnd::Completed)
    }
}

struct Providers;

#[async_trait::async_trait]
impl ProviderService for Providers {
    fn list(&self) -> ark_contract::ProviderListResponse {
        ark_contract::ProviderListResponse::default()
    }

    fn get(&self, _id: &str) -> ark_common::Result<ark_contract::ProviderEntry> {
        unreachable!("not used by this test")
    }

    fn upsert(
        &self,
        _entry: ark_contract::ProviderEntry,
        _make_default: bool,
    ) -> ark_common::Result<ark_contract::ProviderListResponse> {
        unreachable!("not used by this test")
    }

    fn delete(&self, _id: &str) -> ark_common::Result<ark_contract::ProviderListResponse> {
        unreachable!("not used by this test")
    }

    fn set_default(
        &self,
        _id: &str,
        _model: Option<&str>,
    ) -> ark_common::Result<ark_contract::ProviderListResponse> {
        unreachable!("not used by this test")
    }

    async fn test(
        &self,
        _request: ark_contract::ProviderTestRequest,
    ) -> ark_contract::ProviderTestResponse {
        unreachable!("not used by this test")
    }

    async fn models(
        &self,
        _request: ark_contract::ProviderModelCatalogRequest,
    ) -> ark_common::Result<ark_contract::ProviderModelCatalogResponse> {
        unreachable!("not used by this test")
    }

    fn resolve_endpoint(&self, id: Option<&str>) -> Option<ModelEndpoint> {
        (id == Some("known")).then(|| ModelEndpoint {
            provider_id: "known".to_string(),
            model: "model".to_string(),
            base_url: "https://example.invalid".to_string(),
            api_key: "secret".to_string(),
            endpoint_format: "openai".to_string(),
            custom_endpoint: None,
            max_tokens: None,
            temperature: None,
            reasoning_effort: None,
        })
    }
}

fn state(data_dir: &std::path::Path) -> AppState {
    let sessions =
        Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"));
    let store: Arc<dyn SessionStore> = sessions;
    AppState::new(
        store,
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig::default())),
        Arc::new(UsageRecorder::new()),
        Arc::new(NoopRunner),
        data_dir.to_path_buf(),
        "test",
    )
    .with_base_services(BaseServices::new().with_providers(Arc::new(Providers)))
}

/// A fresh random data directory, removed when the guard drops.
///
/// `tempfile` names it uniquely, so two tests running in parallel can never
/// share one store directory or its lock file.
fn test_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("ark-thread-{tag}-"))
        .tempdir()
        .expect("temp dir is creatable")
}

mod list_and_projection_tests;
mod runtime_override_tests;
mod workspace_tests;
