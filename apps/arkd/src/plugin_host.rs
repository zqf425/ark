//! Local plugin host assembly (design §7.7, wiring requirement §组合根装配).
//!
//! This module is deliberately mechanical: it builds the buses, hands them to
//! `ark_plugin::load_host`, and exposes the result. Every rule about *which*
//! plugins run, what they may reach, and what a failure costs lives in
//! `ark-plugin`; nothing here branches on a plugin id.
//!
//! ## What arkd supplies and what it does not
//!
//! The plugin handles need log/trace/metric/notify sinks. There is no dedicated
//! observability service in this build, so those are thin adapters onto
//! `tracing` — the same sink the rest of the process writes to. They carry the
//! plugin id the host already bound, so a plugin cannot forge another's lines.
//!
//! ## Load once
//!
//! Plugins are read at startup and never reloaded (no hot reload, no per-plugin
//! enable flag in this version). `unload` exists for tests and for a future
//! route; the assembly itself does not call it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ark_common::Result;
use ark_http::{CredentialVault, HttpBus, HttpPolicy, MemoryDiagnostics};
use ark_mcp::{McpBus, PluginTool};
use ark_plugin::{load_host, HostHandles, PluginHost};
use ark_ports::services::LogRecord;
use ark_ports::{
    HttpCapability, LogService, McpCapability, MetricService, NotifyService, Tool, TraceService,
    UiCapability,
};
use ark_ui::UiBus;

/// Directory under `data_dir` that holds one folder per plugin.
pub const PLUGINS_DIR: &str = "plugins";

/// Bound on retained HTTP diagnostics; only denials are reported.
const HTTP_DIAGNOSTIC_CAPACITY: usize = 256;

/// The assembled host plus the bus its tools were published to.
pub struct PluginRuntime {
    host: Arc<PluginHost>,
    mcp: Arc<McpBus>,
    root: PathBuf,
}

impl std::fmt::Debug for PluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRuntime")
            .field("root", &self.root)
            .field("report", self.host.report())
            .finish_non_exhaustive()
    }
}

impl PluginRuntime {
    /// The report behind `GET /v1/runtime/plugins`.
    pub fn host(&self) -> &Arc<PluginHost> {
        &self.host
    }

    /// Tools to merge into the `mcp` tool group.
    ///
    /// Built from the bus catalog *after* loading, so a plugin that failed to
    /// register contributes nothing here. `replace_group` takes the whole group,
    /// so the caller passes external MCP adapters alongside these.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.mcp
            .catalog()
            .tools
            .into_iter()
            .filter_map(|spec| {
                PluginTool::new(
                    Arc::clone(&self.mcp),
                    &spec.plugin_id,
                    &spec.tool,
                    spec.schema,
                )
                .map(|tool| Arc::new(tool) as Arc<dyn Tool>)
                .map_err(|err| {
                    // A spec the bus accepted but the adapter rejects would be a
                    // contract bug, not a plugin bug: surface it in the log
                    // rather than dropping the tool silently.
                    tracing::warn!(
                        plugin = %spec.plugin_id,
                        tool = %spec.tool,
                        error = %err.message,
                        "skipping plugin tool that failed to adapt"
                    );
                })
                .ok()
            })
            .collect()
    }

    /// Where plugins were read from.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Build the HTTP egress bus and the plugin host rooted at `<data_dir>/plugins`.
///
/// Returns a runtime whose report is already populated. A missing plugin
/// directory is not an error: this build simply has no plugins installed.
///
/// Plugins are given no `EventSink`. The platform handle would expose the raw
/// store, and a plugin could then append events to any thread it names — a
/// capability nothing in this version needs, so it stays absent rather than
/// becoming a hole the isolation model has to explain.
pub fn assemble(data_dir: &Path, ui_bus: &Arc<UiBus>, mcp: Arc<McpBus>) -> Result<PluginRuntime> {
    let root = data_dir.join(PLUGINS_DIR);
    std::fs::create_dir_all(&root)?;

    let http_diagnostics = Arc::new(MemoryDiagnostics::new(HTTP_DIAGNOSTIC_CAPACITY));
    let vault = Arc::new(CredentialVault::new());
    let http = Arc::new(HttpBus::new(
        HttpPolicy::default(),
        Arc::clone(&vault),
        http_diagnostics.clone(),
    )?);

    let handles = HostHandles {
        http: Arc::clone(&http) as Arc<dyn HttpCapability>,
        mcp: Arc::clone(&mcp) as Arc<dyn McpCapability>,
        ui: Arc::clone(ui_bus) as Arc<dyn UiCapability>,
        ui_bus: Some(Arc::clone(ui_bus)),
        mcp_bus: Some(Arc::clone(&mcp)),
        vault,
        log: Arc::new(TracingLog),
        cfg_store: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
        kv_store: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
        plugins_root: root.clone(),
        trace: Arc::new(TracingTrace),
        metrics: Arc::new(TracingMetrics),
        notify: Arc::new(TracingNotify),
        events: None,
    };

    let host = load_host(&root, handles, http, http_diagnostics)?.with_mcp_bus(Arc::clone(&mcp));
    log_report(&host, &root);

    Ok(PluginRuntime {
        host: Arc::new(host),
        mcp,
        root,
    })
}

/// One boot line per outcome, so a skipped plugin is visible without an HTTP
/// call. The full detail stays available from the diagnostics route.
fn log_report(host: &PluginHost, root: &Path) {
    let report = host.report();
    tracing::info!(
        root = %root.display(),
        loaded = ?report.loaded,
        failed = ?report.failed,
        skipped = ?report.skipped,
        "loaded local plugins"
    );
    for diagnostic in host.diagnostics() {
        tracing::warn!(
            kind = %diagnostic.kind,
            plugin = %diagnostic.plugin_id,
            version = %diagnostic.version,
            detail = %diagnostic.detail,
            "plugin diagnostic"
        );
    }
}

/// Plugin log lines, tagged with the host-bound plugin id.
struct TracingLog;

impl LogService for TracingLog {
    fn log(&self, record: LogRecord) {
        tracing::info!(
            target: "plugin",
            plugin = %record.plugin,
            level = %record.level,
            fields = ?record.fields,
            "{}",
            record.message
        );
    }
}

struct TracingTrace;

impl TraceService for TracingTrace {
    fn span(&self, name: &str, fields: BTreeMap<String, String>) {
        tracing::debug!(target: "plugin", span = %name, fields = ?fields, "plugin span");
    }
}

struct TracingMetrics;

impl MetricService for TracingMetrics {
    fn counter(&self, name: &str, value: u64) {
        tracing::debug!(target: "plugin", counter = %name, value, "plugin metric");
    }
}

struct TracingNotify;

impl NotifyService for TracingNotify {
    fn notify(&self, title: &str, body: &str) -> Result<()> {
        tracing::info!(target: "plugin", title = %title, "{}", body);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
