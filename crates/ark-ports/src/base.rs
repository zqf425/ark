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
    MemoryDiagnosticsResponse, MemoryEntry, MemoryListResponse, PluginEnableRequest,
    PluginHostReport, PluginProvisionView, ProviderEntry, ProviderListResponse,
    ProviderModelCatalogRequest, ProviderModelCatalogResponse, ProviderTestRequest,
    ProviderTestResponse, SkillDiagnostics, SkillTurnResolution, ThreadSkillsResponse,
    UpdateMemoryRequest, WorkspaceDiff, WorkspaceFilePreview, WorkspaceStatus, WorkspaceView,
};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bus::UiCapability;
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
    /// Reasoning-effort override from the thread runtime settings.
    pub reasoning_effort: Option<String>,
}

/// Model provider management (design §7.2 / §7.15).
#[async_trait]
pub trait ProviderService: Send + Sync {
    fn list(&self) -> ProviderListResponse;

    /// Full edit read, including the stored `api_key`. Only the settings edit
    /// form may call this; `list()` stays deliberately redacted.
    fn get(&self, id: &str) -> Result<ProviderEntry>;

    fn upsert(&self, entry: ProviderEntry, make_default: bool) -> Result<ProviderListResponse>;

    fn delete(&self, id: &str) -> Result<ProviderListResponse>;

    /// Name the default provider, optionally pinning one of its models.
    /// `model` must be the entry's stored model or an inventory row id.
    fn set_default(&self, id: &str, model: Option<&str>) -> Result<ProviderListResponse>;

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

    /// Bounded read-only preview of one file under `root`
    /// (requirements/workspace-file-preview.md). Containment against `root` is
    /// the implementor's duty.
    fn read_file(&self, root: &Path, path: &str) -> Result<WorkspaceFilePreview>;

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

/// The local plugin host (design §7.7).
///
/// `ark-server` must answer `GET /v1/runtime/plugins` without linking the crate
/// that loads plugins, so the read side lives behind this trait. The host loads
/// once at startup; the only runtime mutation it serves is the governed
/// provision enable flow (requirements/plugin-provisioning.md): detect the
/// declared tool, optionally install it behind explicit user consent, and
/// register the declared external MCP server.
#[async_trait]
pub trait PluginHostService: Send + Sync {
    /// Every plugin the host currently serves, plus startup and egress
    /// diagnostics. Contributions are read from the buses that own them, so a
    /// page or tool appears only while it is actually resolvable.
    fn report(&self) -> PluginHostReport;

    /// Provision state of one plugin. Validation error when the plugin exists
    /// but declares no `provision` section.
    fn provision_status(&self, plugin_id: &str) -> Result<PluginProvisionView>;

    /// Run the enable flow: detect, optionally install (only with the user
    /// consent flag), then register the declared MCP server. Every step is
    /// host-governed; the plugin contributes declarations only.
    async fn enable(
        &self,
        plugin_id: &str,
        request: PluginEnableRequest,
    ) -> Result<PluginProvisionView>;
}

/// Native folder chooser for local UI flows (workspace switch, thread runtime
/// override).
///
/// Blocking on purpose: a native dialog is modal, so the HTTP boundary must
/// wrap the call in `spawn_blocking`. `None` means the user cancelled. The
/// returned path is a real existing directory the user picked; workspace
/// validation stays with `WorkspaceService`. The port is `None` in headless or
/// remote builds — those must answer `runtime_unavailable` instead of popping
/// a dialog on a machine nobody is looking at.
pub trait FolderDialogService: Send + Sync {
    fn pick_folder(&self) -> Option<PathBuf>;
}

/// One-shot model call for background jobs (design §7.9).
///
/// The turn-time model choice stays inside the composition root's turn runner;
/// this port exists so `ark-server` can run *small auxiliary* model calls
/// (thread-title generation) without learning which provider is live. Like
/// [`TurnRunner`](crate::exec), it is a seam the composition root fills in; a
/// build without it simply never names titles. Implementations own provider
/// resolution, metering, hooks and the timeout budget.
#[async_trait]
pub trait TitleModel: Send + Sync {
    /// Generate one short title from the conversation excerpt. Returns `None`
    /// when no usable model is configured; an error means the call failed.
    async fn generate_title(&self, excerpt: &str) -> Result<Option<String>>;
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
    /// The UI catalog. Absent in a build without a UI bus, which makes
    /// `GET /v1/runtime/capabilities` report no pages instead of inventing them.
    pub ui: Option<Arc<dyn UiCapability>>,
    /// The local plugin host. Absent in a build that loads no plugins, which
    /// makes `GET /v1/runtime/plugins` report an unavailable service rather
    /// than an empty, indistinguishable plugin list.
    pub plugins: Option<Arc<dyn PluginHostService>>,
    /// Native folder dialog. Absent in headless/remote builds, which makes
    /// `POST /v1/workspace/pick` report `runtime_unavailable` rather than
    /// opening a dialog where nobody can click it.
    pub folder_dialog: Option<Arc<dyn FolderDialogService>>,
    /// One-shot background model calls (thread titles). Absent in builds
    /// without a live provider; titles then simply stay on their placeholder.
    pub title_model: Option<Arc<dyn TitleModel>>,
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

    pub fn with_ui(mut self, service: Arc<dyn UiCapability>) -> Self {
        self.ui = Some(service);
        self
    }

    pub fn with_plugins(mut self, service: Arc<dyn PluginHostService>) -> Self {
        self.plugins = Some(service);
        self
    }

    pub fn with_folder_dialog(mut self, service: Arc<dyn FolderDialogService>) -> Self {
        self.folder_dialog = Some(service);
        self
    }

    pub fn with_title_model(mut self, service: Arc<dyn TitleModel>) -> Self {
        self.title_model = Some(service);
        self
    }
}
