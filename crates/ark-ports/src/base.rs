//! Platform capability ports: providers, skills, workspace, MCP servers.
//!
//! These four are *platform* services (design §7.5.4), so `ark-server` must be
//! able to serve them without depending on the crates that implement them. The
//! traits here are that seam: `arkd` wires the concrete stores, the HTTP layer
//! only ever sees these.
//!
//! Shapes are the wire DTOs from `ark-contract`, so a route is a straight
//! pass-through and cannot drift from what the GUI parses.

use ark_common::Result;
use ark_contract::{
    CreateMemoryRequest, McpServerEntry, McpServerListResponse, McpServerTestResponse,
    MemoryDiagnosticsResponse, MemoryEntry, MemoryListResponse, ProviderEntry,
    ProviderListResponse, ProviderModelCatalogRequest, ProviderModelCatalogResponse,
    ProviderTestRequest, ProviderTestResponse, SkillDiagnostics, SkillTurnResolution,
    ThreadSkillsResponse, UpdateMemoryRequest, WorkspaceDiff, WorkspaceStatus, WorkspaceView,
};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

use crate::tool::Tool;

/// Provider table, resolved into a client config.
///
/// `api_key` travels in this struct because `arkd` is the only consumer and it
/// builds the client immediately; it never reaches a log or an HTTP response.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelEndpoint {
    pub provider_id: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub endpoint_format: String,
    pub custom_endpoint: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// Model provider management (design §7.2 / §7.15).
#[async_trait]
pub trait ProviderService: Send + Sync {
    fn list(&self) -> ProviderListResponse;

    fn upsert(&self, entry: ProviderEntry, make_default: bool) -> Result<ProviderListResponse>;

    fn delete(&self, id: &str) -> Result<ProviderListResponse>;

    fn set_default(&self, id: &str) -> Result<ProviderListResponse>;

    /// Real connectivity probe; `request.entry` tests an unsaved draft.
    async fn test(&self, request: ProviderTestRequest) -> ProviderTestResponse;

    /// Query a provider's model catalog for the settings picker.
    ///
    /// Parameter: `request` identifies a saved provider or carries an unsaved
    /// connection draft. Returns the normalized, explicitly reported model
    /// metadata, or a structured validation/transport error.
    async fn models(
        &self,
        request: ProviderModelCatalogRequest,
    ) -> Result<ProviderModelCatalogResponse>;

    /// Resolve the endpoint a turn should use. `None` when nothing usable is
    /// configured, so `arkd` can fall back deliberately instead of guessing.
    fn resolve_endpoint(&self, id: Option<&str>) -> Option<ModelEndpoint>;
}

/// Skill discovery and activation (design §7.14).
pub trait SkillService: Send + Sync {
    fn list(&self) -> SkillDiagnostics;

    /// Re-scan every root and return the fresh diagnostics.
    fn reload(&self) -> SkillDiagnostics;

    fn thread_view(&self, extra_roots: &[PathBuf]) -> ThreadSkillsResponse;

    fn resolve_for_turn(&self, prompt: &str) -> SkillTurnResolution;
}

/// Workspace inspection (design §7.9).
pub trait WorkspaceService: Send + Sync {
    /// Validate a candidate workspace path before it is stored.
    fn validate(&self, raw: &str) -> Result<PathBuf>;

    fn status(&self, path: &str) -> WorkspaceStatus;

    /// Snapshot of the process-owned active workspace.
    fn view(&self) -> WorkspaceView;

    /// Switch after the HTTP boundary has checked that no parent/child runs are
    /// active. Returns a conflict instead of cancelling work.
    fn switch(&self, path: &str, active_runs: usize) -> Result<WorkspaceView>;

    /// Bounded Git diff for the active canonical root.
    fn diff(&self) -> Result<WorkspaceDiff>;
}

/// External MCP server connections (design §7.6).
#[async_trait]
pub trait McpServerService: Send + Sync {
    fn list(&self) -> McpServerListResponse;

    async fn upsert(&self, entry: McpServerEntry) -> Result<McpServerListResponse>;

    async fn delete(&self, id: &str) -> Result<McpServerListResponse>;

    /// Handshake + `tools/list`, with a bounded timeout.
    async fn test(&self, id: &str) -> McpServerTestResponse;

    /// Connect every enabled server and return the tool adapters to register.
    /// A single failing server is reported through `list()`, never raised here.
    async fn connect_all(&self) -> Vec<Arc<dyn Tool>>;
}

/// Durable long-term user memory shared across sessions.
pub trait MemoryService: Send + Sync {
    fn list(&self) -> Result<MemoryListResponse>;

    fn create(&self, request: CreateMemoryRequest) -> Result<MemoryEntry>;

    fn update(&self, id: &str, request: UpdateMemoryRequest) -> Result<MemoryEntry>;

    fn delete(&self, id: &str) -> Result<()>;

    fn diagnostics(&self) -> Result<MemoryDiagnosticsResponse>;
}

/// The platform services, as one injectable bundle.
///
/// A field is `Option` so a minimal build (or a test) can serve only some of
/// them; the route then answers a structured "not available" instead of
/// pretending the feature exists.
#[derive(Clone, Default)]
pub struct BaseServices {
    pub providers: Option<Arc<dyn ProviderService>>,
    pub skills: Option<Arc<dyn SkillService>>,
    pub workspace: Option<Arc<dyn WorkspaceService>>,
    pub mcp: Option<Arc<dyn McpServerService>>,
    pub memory: Option<Arc<dyn MemoryService>>,
}

impl BaseServices {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_providers(mut self, service: Arc<dyn ProviderService>) -> Self {
        self.providers = Some(service);
        self
    }

    pub fn with_skills(mut self, service: Arc<dyn SkillService>) -> Self {
        self.skills = Some(service);
        self
    }

    pub fn with_workspace(mut self, service: Arc<dyn WorkspaceService>) -> Self {
        self.workspace = Some(service);
        self
    }

    pub fn with_mcp(mut self, service: Arc<dyn McpServerService>) -> Self {
        self.mcp = Some(service);
        self
    }

    pub fn with_memory(mut self, service: Arc<dyn MemoryService>) -> Self {
        self.memory = Some(service);
        self
    }
}
