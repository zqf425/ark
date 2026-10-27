//! Host-built plugin context (design 7.7.1, 7.5.4, 7.5.5).
//!
//! The context is the **only** registration entry point. Every handle inside it
//! was constructed by the host with `identity.plugin_id()` already baked in, and
//! no method here takes an owner argument. A plugin therefore cannot address
//! another plugin's config, KV keys, file root, credentials or tool namespace,
//! even if it knows their ids.

use crate::domain::PluginIdentity;
use crate::service::isolation::{
    ScopedConfig, ScopedCredentials, ScopedFiles, ScopedKv, ScopedLog, ScopedMetrics, ScopedNotify,
    ScopedTrace,
};
use ark_common::{ArkError, Result};
use ark_contract::{Capability, PageSpec, PanelSlot, PanelSpec};
use ark_mcp::{ExposureHandle, McpBus};
use ark_ports::{
    CredentialService, EventSink, FileService, HttpCapability, LogService, McpCapability,
    McpToolSpec, MetricService, NotifyService, TraceService, UiCapability,
};
use ark_ui::{PageRegistration, PanelRegistration, UiBus};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Raw host handles, before binding to one plugin identity.
///
/// The loader clones these for every plugin; the plugin only ever sees the
/// scoped views built in [`PluginContext`].
#[derive(Clone)]
pub struct HostHandles {
    pub http: Arc<dyn HttpCapability>,
    pub mcp: Arc<dyn McpCapability>,
    pub ui: Arc<dyn UiCapability>,
    /// Concrete buses are kept so registration can be owner-scoped, which the
    /// port traits deliberately do not allow.
    pub ui_bus: Option<Arc<UiBus>>,
    pub mcp_bus: Option<Arc<McpBus>>,
    /// Vault shared with the HTTP capability: credential references resolve
    /// against the same secrets the egress injects.
    pub vault: Arc<ark_http::CredentialVault>,
    pub log: Arc<dyn LogService>,
    pub cfg_store: Arc<Mutex<BTreeMap<String, serde_json::Value>>>,
    pub kv_store: Arc<Mutex<BTreeMap<String, String>>>,
    /// `data/plugins`; each plugin is rooted at `<plugins_root>/<plugin_id>`.
    pub plugins_root: PathBuf,
    pub trace: Arc<dyn TraceService>,
    pub metrics: Arc<dyn MetricService>,
    pub notify: Arc<dyn NotifyService>,
    pub events: Option<Arc<dyn EventSink>>,
}

impl std::fmt::Debug for HostHandles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostHandles")
            .field("plugins_root", &self.plugins_root)
            .field("events", &self.events.is_some())
            .finish_non_exhaustive()
    }
}

/// Everything a plugin receives.
pub struct PluginContext {
    identity: PluginIdentity,
    http: Arc<dyn HttpCapability>,
    mcp: Arc<dyn McpCapability>,
    ui: Arc<dyn UiCapability>,
    ui_bus: Option<Arc<UiBus>>,
    mcp_bus: Option<Arc<McpBus>>,
    log_service: Arc<dyn LogService>,
    cfg: ScopedConfig,
    cred: Arc<dyn CredentialService>,
    kv: Arc<ScopedKv>,
    files: Arc<dyn FileService>,
    trace: Arc<dyn TraceService>,
    metrics: Arc<dyn MetricService>,
    notify: Arc<dyn NotifyService>,
    events: Option<Arc<dyn EventSink>>,
}

impl std::fmt::Debug for PluginContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginContext")
            .field("identity", &self.identity.label())
            .field("events", &self.events.is_some())
            .finish_non_exhaustive()
    }
}

impl PluginContext {
    /// Bind the host handles to one identity. `pub(crate)`: only the loader can
    /// create a context, so a plugin cannot build one for another plugin id.
    pub(crate) fn host_build(identity: PluginIdentity, handles: &HostHandles) -> Self {
        let owner = identity.plugin_id().to_string();
        let log = Arc::new(ScopedLog::new(&owner, handles.log.clone()));
        let cred = Arc::new(ScopedCredentials::new(&owner, handles.vault.clone()));
        Self {
            identity,
            http: handles.http.clone(),
            mcp: handles.mcp.clone(),
            ui: handles.ui.clone(),
            ui_bus: handles.ui_bus.clone(),
            mcp_bus: handles.mcp_bus.clone(),
            log_service: log,
            cfg: ScopedConfig::with_store(&owner, handles.cfg_store.clone()),
            cred,
            kv: Arc::new(ScopedKv::with_store(&owner, handles.kv_store.clone())),
            files: Arc::new(ScopedFiles::under(&handles.plugins_root, &owner)),
            trace: Arc::new(ScopedTrace::new(&owner, handles.trace.clone())),
            metrics: Arc::new(ScopedMetrics::new(&owner, handles.metrics.clone())),
            notify: Arc::new(ScopedNotify::new(&owner, handles.notify.clone())),
            events: handles.events.clone(),
        }
    }

    /// Host-issued identity. Read-only: there is no setter.
    pub fn identity(&self) -> &PluginIdentity {
        &self.identity
    }

    pub fn plugin_id(&self) -> &str {
        self.identity.plugin_id()
    }

    pub fn version(&self) -> &str {
        self.identity.version()
    }

    pub fn http(&self) -> &Arc<dyn HttpCapability> {
        &self.http
    }

    pub fn mcp(&self) -> &Arc<dyn McpCapability> {
        &self.mcp
    }

    pub fn ui(&self) -> &Arc<dyn UiCapability> {
        &self.ui
    }

    /// Scoped log: every record is stamped with this plugin id.
    pub fn log(&self) -> &Arc<dyn LogService> {
        &self.log_service
    }

    /// Private config view: keys are physically `{plugin_id}.{key}`.
    pub fn cfg(&self) -> &ScopedConfig {
        &self.cfg
    }

    /// Credential handle: no plaintext read API.
    pub fn cred(&self) -> &Arc<dyn CredentialService> {
        &self.cred
    }

    /// Private KV view: key space fixed to this plugin.
    pub fn kv(&self) -> &Arc<ScopedKv> {
        &self.kv
    }

    /// Private files: root locked to `data/plugins/<plugin_id>/`.
    pub fn files(&self) -> &Arc<dyn FileService> {
        &self.files
    }

    pub fn trace(&self) -> &Arc<dyn TraceService> {
        &self.trace
    }

    pub fn metrics(&self) -> &Arc<dyn MetricService> {
        &self.metrics
    }

    pub fn notify(&self) -> &Arc<dyn NotifyService> {
        &self.notify
    }

    /// Optional global event subscription, present only after a grant.
    pub fn events(&self) -> Option<&Arc<dyn EventSink>> {
        self.events.as_ref()
    }

    /// Register a page owned by this plugin; `Drop` unregisters it.
    pub fn register_page(&self, page: PageSpec) -> Result<PageRegistration> {
        let bus = self.ui_bus.as_ref().ok_or_else(ui_unavailable)?;
        bus.register_page(self.plugin_id(), page)
    }

    /// Register a panel owned by this plugin; `Drop` unregisters it.
    pub fn register_panel(&self, slot: PanelSlot, panel: PanelSpec) -> Result<PanelRegistration> {
        let bus = self.ui_bus.as_ref().ok_or_else(ui_unavailable)?;
        bus.register_panel(self.plugin_id(), slot, panel)
    }

    /// Publish MCP tool specs under `<plugin_id>.<tool>`; `Drop` unregisters.
    ///
    /// A manifest that tries to publish for another plugin is rejected here, not
    /// silently rewritten: the plugin id inside a spec is part of its public
    /// name, so a mismatch is a bug worth failing on.
    pub fn expose_tools(&self, tools: Vec<McpToolSpec>) -> Result<ExposureHandle> {
        let bus = self.mcp_bus.as_ref().ok_or_else(mcp_unavailable)?;
        for spec in &tools {
            if spec.plugin_id != self.plugin_id() {
                return Err(ArkError::new(
                    ark_common::ArkErrorCode::Unauthorized,
                    format!(
                        "plugin {} cannot expose tools for {}",
                        self.plugin_id(),
                        spec.plugin_id
                    ),
                )
                .with_detail("plugin_identity_mismatch"));
            }
        }
        bus.expose_specs(self.plugin_id(), tools)
    }

    /// Declare this plugin's `mcp.expose` / `mcp.consume` lists.
    pub fn declare_mcp(&self, expose: &[String], consume: &[String]) -> Result<()> {
        let bus = self.mcp_bus.as_ref().ok_or_else(mcp_unavailable)?;
        bus.declare_visibility(self.plugin_id(), expose, consume)
    }

    /// Does the host grant this plugin the capability?
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.identity.grant().allows(capability)
    }

    /// Fail closed when a platform service is absent (design 7.5.4): the caller
    /// skips this plugin with a diagnostic instead of running half-wired.
    pub fn require_service<T>(&self, service: Option<T>, name: &str) -> Result<T> {
        service.ok_or_else(|| {
            ArkError::new(
                ark_common::ArkErrorCode::RuntimeUnavailable,
                format!("platform service {name} is not available"),
            )
            .with_detail("plugin_service_missing")
        })
    }
}

fn ui_unavailable() -> ArkError {
    ArkError::internal("ui bus does not support scoped registration")
        .with_detail("plugin_ui_unavailable")
}

fn mcp_unavailable() -> ArkError {
    ArkError::internal("mcp bus does not support scoped exposure")
        .with_detail("plugin_mcp_unavailable")
}
