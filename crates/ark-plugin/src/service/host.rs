//! Host-side plugin loader: the one place that turns a directory into a running
//! set of plugins (design 7.7.3).
//!
//! Three policies meet here, in this order:
//!
//! 1. [`scan`] reads `<root>/<plugin_id>/plugin.json` and validates each file.
//!    A broken file is a diagnostic, never a startup failure.
//! 2. [`plan_local_load`] partitions the survivors into a dependency order plus
//!    explained drops, so a cycle or a missing service takes down only the
//!    plugins that depend on it.
//! 3. [`PluginRegistry::load`] binds identity and runs `register`. A failing
//!    `register` skips that plugin and its dependents.
//!
//! Egress rights are declared *before* any plugin code runs and revoked for
//! every plugin that did not start, so a failed plugin cannot keep a live
//! allowlist entry behind.

use crate::domain::isolation::{plan_local_load, PluginDrop};
use crate::domain::manifest::PluginManifest;
use crate::service::context::HostHandles;
use crate::service::directory::{scan, DirectoryPlugin, ScannedPlugin};
use crate::service::hook_audit::{HookAuditRecord, InMemoryHookAudit};
use crate::service::plugin::Plugin;
use crate::service::provision::{OsCommandRunner, ProvisionFlow, ProvisionSpec};
use crate::service::registry::{
    LoadReport, MemoryPluginDiagnostics, PluginDiagnostic, PluginDiagnosticSink, PluginRegistry,
};
use ark_common::{ArkError, Result};
use ark_contract::{
    PluginDiagnosticView, PluginEnableRequest, PluginEntry, PluginHostReport, PluginProvisionView,
};
use ark_http::{HttpBus, InMemoryDiagnosticSink};
use ark_mcp::McpBus;
use ark_ports::{HttpCapability, McpServerService};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// A loaded plugin host: the registry plus everything needed to report on it.
pub struct PluginHost {
    registry: Mutex<PluginRegistry>,
    diagnostics: Arc<MemoryPluginDiagnostics>,
    http_diagnostics: Arc<InMemoryDiagnosticSink>,
    /// Bounded ring for hook decisions (requirements/plugin-hook-bus.md).
    /// Owned here so the P2 broker multiplexer can write without rewiring;
    /// until then it stays empty and the report field is an empty list.
    hook_audit: Arc<InMemoryHookAudit>,
    http: Arc<HttpBus>,
    mcp: Option<Arc<McpBus>>,
    /// Declarative enable flows (requirements/plugin-provisioning.md). `None`
    /// when no loaded plugin declares a `provision` section.
    provision: Option<ProvisionFlow>,
    report: LoadReport,
    root: PathBuf,
}

impl std::fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHost")
            .field("root", &self.root)
            .field("loaded", &self.report.loaded)
            .field("failed", &self.report.failed)
            .field("skipped", &self.report.skipped)
            .finish_non_exhaustive()
    }
}

/// Bounded audit retention: enough to answer "why was this refused" for a
/// support session, small enough that a chatty plugin cannot grow the process.
const HOOK_AUDIT_CAPACITY: usize = 128;

/// Load every plugin under `root`.
///
/// `http` is the same bus the plugins receive; it is a separate argument because
/// egress registration is host-only and must stay unreachable from a plugin.
pub fn load_host(
    root: &Path,
    handles: HostHandles,
    http: Arc<HttpBus>,
    http_diagnostics: Arc<InMemoryDiagnosticSink>,
) -> Result<PluginHost> {
    let (mut scanned, scan_diagnostics) = scan(root);
    let diagnostics_sink = Arc::new(MemoryPluginDiagnostics::new());
    for record in &scan_diagnostics {
        diagnostics_sink.record(record.clone());
    }

    // A directory plugin is declarative: it has no code to receive a hook
    // callback, so a `hooks` section cannot be honoured and the plugin is
    // skipped before planning (fail closed, requirements/plugin-hook-bus.md).
    // Removing it here also drops its dependents for the ordinary
    // missing-dependency reason instead of leaving a phantom in the topology.
    let version_of = |item: &ScannedPlugin| item.manifest.version.to_string();
    scanned.retain(|item| {
        if item.manifest.hooks.is_some() {
            diagnostics_sink.record(PluginDiagnostic::new(
                "plugin_skipped",
                &item.manifest.id,
                &version_of(item),
                "directory plugins cannot declare hooks; the section is reserved                  for builtin plugins",
            ));
            false
        } else {
            true
        }
    });

    let versions = versions_by_id(&scanned);
    let plan = plan_local_load(&manifests(&scanned))?;
    for dropped in &plan.dropped {
        diagnostics_sink.record(drop_diagnostic(dropped, &versions));
    }

    let by_id: BTreeMap<&str, &ScannedPlugin> = scanned
        .iter()
        .map(|item| (item.manifest.id.as_str(), item))
        .collect();
    let ready: Vec<&ScannedPlugin> = plan
        .ready
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).copied())
        .collect();

    // Declare egress before `register` can make a call.
    for item in &ready {
        http.register_plugin(
            &item.manifest.id,
            &item.manifest.version.to_string(),
            &item.manifest.http.allow,
        )?;
    }

    let plugins: Vec<Arc<dyn Plugin>> = ready
        .iter()
        .map(|item| {
            let capability: Arc<dyn HttpCapability> = Arc::clone(&http) as _;
            Arc::new(DirectoryPlugin::new(item, capability)) as Arc<dyn Plugin>
        })
        .collect();

    let mut registry = PluginRegistry::new(handles, diagnostics_sink.clone());
    let report = registry.load(&plugins)?;

    // Fail closed: a plugin that did not start keeps no egress rights.
    for plugin_id in report.failed.iter().chain(report.skipped.iter()) {
        http.unregister_plugin(plugin_id);
    }

    // Collect declarative enable flows (requirements/plugin-provisioning.md).
    // Only plugins that actually started are enableable: a skipped or failed
    // plugin contributes no spec, so its manifest section stays inert.
    let provision_specs: Vec<ProvisionSpec> = ready
        .iter()
        .filter_map(|item| {
            item.declarations.provision.as_ref().map(|section| ProvisionSpec {
                plugin_id: item.manifest.id.clone(),
                plugin_dir: item.directory.clone(),
                section: section.clone(),
            })
        })
        .collect();
    let provision = if provision_specs.is_empty() {
        None
    } else {
        Some(ProvisionFlow::load(
            provision_specs,
            Arc::new(OsCommandRunner),
        ))
    };

    Ok(PluginHost {
        registry: Mutex::new(registry),
        diagnostics: diagnostics_sink,
        http_diagnostics,
        hook_audit: Arc::new(InMemoryHookAudit::new(HOOK_AUDIT_CAPACITY)),
        http,
        mcp: None,
        provision,
        report,
        root: root.to_path_buf(),
    })
}

fn manifests(scanned: &[ScannedPlugin]) -> Vec<PluginManifest> {
    scanned.iter().map(|item| item.manifest.clone()).collect()
}

fn versions_by_id(scanned: &[ScannedPlugin]) -> BTreeMap<String, String> {
    scanned
        .iter()
        .map(|item| (item.manifest.id.clone(), item.manifest.version.to_string()))
        .collect()
}

impl PluginHost {
    /// Attach the MCP bus, so the report lists the tools actually published.
    pub fn with_mcp_bus(mut self, mcp: Arc<McpBus>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    /// Attach the external MCP server registry the provision flow registers
    /// into (requirements/plugin-provisioning.md). Composition-root wiring
    /// only; never reachable from a plugin.
    pub fn attach_mcp_servers(&self, service: Arc<dyn McpServerService>) {
        if let Some(flow) = self.provision.as_ref() {
            flow.attach_mcp_servers(service);
        }
    }

    /// Where plugins were read from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The startup outcome, for the boot log.
    pub fn report(&self) -> &LoadReport {
        &self.report
    }

    /// Startup diagnostics recorded so far, in load order.
    pub fn diagnostics(&self) -> Vec<PluginDiagnostic> {
        self.diagnostics.records()
    }

    /// Unload one plugin now: pages, panels, tools and egress all go away.
    pub fn unload(&self, plugin_id: &str) -> bool {
        let removed = self.lock().unload(plugin_id);
        if removed {
            self.http.unregister_plugin(plugin_id);
        }
        removed
    }

    /// `GET /v1/runtime/plugins`.
    ///
    /// Contributions are read from the handles the plugin still owns, so an
    /// unloaded plugin cannot appear here with a stale page or tool.
    pub fn host_report(&self) -> PluginHostReport {
        let registry = self.lock();
        let plugins = registry
            .loaded_ids()
            .into_iter()
            .filter_map(|id| self.entry(&registry, &id))
            .collect();
        drop(registry);
        PluginHostReport {
            plugins,
            failed: self.report.failed.clone(),
            skipped: self.report.skipped.clone(),
            diagnostics: self
                .diagnostics
                .records()
                .into_iter()
                .map(|record| PluginDiagnosticView {
                    kind: record.kind,
                    plugin_id: record.plugin_id,
                    version: record.version,
                    detail: record.detail,
                })
                .collect(),
            http_denials: self.http_denials(),
            hook_audit: self
                .hook_audit
                .records()
                .into_iter()
                .map(hook_audit_view)
                .collect(),
        }
    }

    /// The audit sink the hook broker writes into. Host-only: a plugin can read
    /// its own decisions on the wire report, never write the audit log.
    pub fn hook_audit_sink(&self) -> Arc<InMemoryHookAudit> {
        Arc::clone(&self.hook_audit)
    }

    /// Refused outbound requests, newest last.
    ///
    /// Only denials are exported: successful spans are ordinary telemetry and
    /// would bury the one record an operator actually needs.
    fn http_denials(&self) -> Vec<PluginDiagnosticView> {
        self.http_diagnostics
            .records()
            .into_iter()
            .filter(|record| record.kind == "http_denied")
            .map(|record| PluginDiagnosticView {
                kind: record.kind,
                plugin_id: record.plugin_id,
                version: String::new(),
                detail: match record.reason {
                    Some(reason) => format!(
                        "{} {}: {}",
                        reason.as_str(),
                        record.redacted_url,
                        record.detail.unwrap_or_default()
                    ),
                    None => record.detail.unwrap_or_default(),
                },
            })
            .collect()
    }

    /// One report row, or `None` when the manifest vanished from the registry.
    fn entry(&self, registry: &PluginRegistry, plugin_id: &str) -> Option<PluginEntry> {
        let manifest = registry.manifest(plugin_id)?;
        let registration = registry.registration(plugin_id);
        Some(PluginEntry {
            id: manifest.id.clone(),
            version: manifest.version.to_string(),
            kind: manifest.kind.as_str().to_string(),
            capabilities: manifest.capabilities.clone(),
            pages: registration.map(|r| r.page_ids()).unwrap_or_default(),
            panels: registration.map(|r| r.panel_ids()).unwrap_or_default(),
            tools: self.published_tools(&manifest.id),
            // The declared request, not the grant: a user reads here what the
            // plugin asked for (requirements/plugin-hook-bus.md, 权限与安全).
            hook_points: manifest.hook_points(),
            provision: self.provision.as_ref().and_then(|flow| flow.status(&manifest.id)),
        })
    }

    /// Qualified tool names the bus actually serves for `plugin_id`.
    fn published_tools(&self, plugin_id: &str) -> Vec<String> {
        let Some(mcp) = self.mcp.as_ref() else {
            return Vec::new();
        };
        mcp.catalog()
            .tools
            .into_iter()
            .filter(|spec| spec.plugin_id == plugin_id)
            .map(|spec| ark_mcp::qualify(&spec.plugin_id, &spec.tool))
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, PluginRegistry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl ark_ports::PluginHostService for PluginHost {
    fn report(&self) -> PluginHostReport {
        self.host_report()
    }

    fn provision_status(&self, plugin_id: &str) -> Result<PluginProvisionView> {
        self.provision
            .as_ref()
            .and_then(|flow| flow.status(plugin_id))
            .ok_or_else(|| {
                ArkError::validation(format!("plugin {plugin_id} declares no provision section"))
            })
    }

    async fn enable(
        &self,
        plugin_id: &str,
        request: PluginEnableRequest,
    ) -> Result<PluginProvisionView> {
        let Some(flow) = self.provision.as_ref() else {
            return Err(ArkError::validation(format!(
                "plugin {plugin_id} declares no provision section"
            )));
        };
        flow.enable(plugin_id, request).await
    }
}

/// Fold one audit record into its wire view.
fn hook_audit_view(record: HookAuditRecord) -> ark_contract::HookAuditView {
    ark_contract::HookAuditView {
        kind: record.kind,
        plugin_id: record.plugin_id,
        hook_point: record.hook_point,
        decision: record.decision,
        detail: record.detail,
        duration_ms: record.duration_ms,
    }
}

fn drop_diagnostic(dropped: &PluginDrop, versions: &BTreeMap<String, String>) -> PluginDiagnostic {
    PluginDiagnostic::new(
        &dropped.code,
        &dropped.plugin_id,
        versions
            .get(&dropped.plugin_id)
            .map(String::as_str)
            .unwrap_or_default(),
        dropped.detail.clone(),
    )
}

#[cfg(test)]
mod tests;
