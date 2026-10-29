//! Adapters from the four platform stores to the `ark-ports` capability traits.
//!
//! `ark-server` must not depend on `ark-provider` / `ark-skills` /
//! `ark-workspace` / `ark-mcp` (design §5.2: the boundary talks to ports). This
//! module is the single place that knows both sides, so the wiring stays a
//! straight composition instead of a chain of `if service == ...` branches.
//!
//! Each adapter is deliberately mechanical: it forwards, converts a concrete
//! result into the contract shape, and nothing else. Any real rule lives in the
//! store behind it.

use std::path::PathBuf;
use std::sync::Arc;

use ark_common::Result;
use ark_contract::{
    McpServerEntry, McpServerListResponse, McpServerTestResponse, ProviderEntry,
    ProviderListResponse, ProviderModelCatalogRequest, ProviderModelCatalogResponse,
    ProviderTestRequest, ProviderTestResponse, SkillDiagnostics, SkillTurnResolution,
    ThreadSkillsResponse, WorkspaceDiff, WorkspaceStatus, WorkspaceView,
};
use ark_ports::{
    McpServerService, ModelEndpoint, ProviderService, SkillService, Tool, WorkspaceService,
};
use ark_tool::ToolRegistry;

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

/// Forwards to `ark-provider::ProviderStore`.
pub struct ProviderAdapter {
    inner: Arc<ark_provider::ProviderStore>,
}

impl ProviderAdapter {
    pub fn new(inner: Arc<ark_provider::ProviderStore>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl ProviderService for ProviderAdapter {
    fn list(&self) -> ProviderListResponse {
        self.inner.list()
    }

    fn upsert(&self, entry: ProviderEntry, make_default: bool) -> Result<ProviderListResponse> {
        self.inner.upsert(entry, make_default)
    }

    fn delete(&self, id: &str) -> Result<ProviderListResponse> {
        self.inner.delete(id)
    }

    fn set_default(&self, id: &str) -> Result<ProviderListResponse> {
        self.inner.set_default(id)
    }

    async fn test(&self, request: ProviderTestRequest) -> ProviderTestResponse {
        self.inner.test(request).await
    }

    async fn models(
        &self,
        request: ProviderModelCatalogRequest,
    ) -> Result<ProviderModelCatalogResponse> {
        self.inner.models(request).await
    }

    fn resolve_endpoint(&self, id: Option<&str>) -> Option<ModelEndpoint> {
        let config = self.inner.resolve(id)?;
        Some(ModelEndpoint {
            provider_id: config.provider_id,
            model: config.model,
            base_url: config.base_url,
            api_key: config.api_key,
            endpoint_format: config.endpoint_format.as_str().to_string(),
            custom_endpoint: config.custom_endpoint,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        })
    }
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

/// Forwards to `ark-skills::SkillRuntime`.
pub struct SkillAdapter {
    inner: Arc<ark_skills::SkillRuntime>,
}

impl SkillAdapter {
    pub fn new(inner: Arc<ark_skills::SkillRuntime>) -> Self {
        Self { inner }
    }
}

impl SkillService for SkillAdapter {
    fn list(&self) -> SkillDiagnostics {
        self.inner.list()
    }

    fn reload(&self) -> SkillDiagnostics {
        self.inner.reload()
    }

    fn thread_view(&self, extra_roots: &[PathBuf]) -> ThreadSkillsResponse {
        self.inner.thread_view(extra_roots)
    }

    fn resolve_for_turn(&self, prompt: &str) -> SkillTurnResolution {
        self.inner.resolve_for_turn(prompt)
    }
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

/// Forwards to `ark-workspace::WorkspaceInspector`.
pub struct WorkspaceAdapter {
    inner: Arc<ark_workspace::WorkspaceInspector>,
    roots: Arc<crate::workspace::WorkspaceRoots>,
}

impl WorkspaceAdapter {
    pub fn new(
        inner: Arc<ark_workspace::WorkspaceInspector>,
        roots: Arc<crate::workspace::WorkspaceRoots>,
    ) -> Self {
        Self { inner, roots }
    }
}

impl WorkspaceService for WorkspaceAdapter {
    fn validate(&self, raw: &str) -> Result<PathBuf> {
        self.inner.validate(raw)
    }

    fn status(&self, path: &str) -> WorkspaceStatus {
        self.inner.status(path)
    }

    fn view(&self) -> WorkspaceView {
        self.roots.view()
    }

    fn switch(&self, path: &str, active_runs: usize) -> Result<WorkspaceView> {
        let validated = self.inner.validate(path)?;
        self.roots
            .switch(&validated.display().to_string(), active_runs)
    }

    fn diff(&self) -> Result<WorkspaceDiff> {
        self.inner.diff(&self.roots.lease().canonical_root)
    }
}

// ---------------------------------------------------------------------------
// MCP servers
// ---------------------------------------------------------------------------

/// Forwards to `ark-mcp::McpServerRegistry`.
pub struct McpServerAdapter {
    inner: Arc<ark_mcp::McpServerRegistry>,
    tools: Arc<ToolRegistry>,
}

impl McpServerAdapter {
    pub fn new(inner: Arc<ark_mcp::McpServerRegistry>, tools: Arc<ToolRegistry>) -> Self {
        Self { inner, tools }
    }

    async fn synchronize_tools(&self) -> Result<()> {
        let connected = self.inner.connect_all().await;
        self.tools.replace_group("mcp", connected)
    }
}

#[async_trait::async_trait]
impl McpServerService for McpServerAdapter {
    fn list(&self) -> McpServerListResponse {
        self.inner.list()
    }

    async fn upsert(&self, entry: McpServerEntry) -> Result<McpServerListResponse> {
        let response = self.inner.upsert(entry)?;
        self.synchronize_tools().await?;
        Ok(response)
    }

    async fn delete(&self, id: &str) -> Result<McpServerListResponse> {
        let response = self.inner.delete(id)?;
        self.synchronize_tools().await?;
        Ok(response)
    }

    async fn test(&self, id: &str) -> McpServerTestResponse {
        self.inner.test(id).await
    }

    async fn connect_all(&self) -> Vec<Arc<dyn Tool>> {
        self.inner.connect_all().await
    }
}
