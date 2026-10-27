//! Loader and registry: topology, identity issuance, fail-closed startup
//! (design 7.5.4, 7.7.3).
//!
//! Two failure policies meet here and they are deliberately different:
//!
//! * **Declaration errors** (cycle, missing service, major mismatch, duplicate
//!   id) abort the whole load before any plugin code runs; there is no runtime
//!   fallback that could hide a broken manifest.
//! * **Runtime load errors** (a plugin's `register` returns `Err`) only skip
//!   *that* plugin plus its dependents, recording `plugin_load_failed`, so one
//!   broken plugin cannot poison the whole runtime.

use crate::domain::{
    grant_for, plan_skips, resolve_order, PluginId, PluginIdentity, PluginManifest,
};
use crate::service::context::{HostHandles, PluginContext};
use crate::service::plugin::{Plugin, Registration};
use ark_common::{ArkError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// One startup diagnostic. `plugin_load_failed` is the design-mandated code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    /// `plugin_loaded`, `plugin_load_failed` or `plugin_skipped`.
    pub kind: String,
    pub plugin_id: String,
    pub version: String,
    pub detail: String,
}

impl PluginDiagnostic {
    pub fn new(kind: &str, plugin_id: &str, version: &str, detail: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            plugin_id: plugin_id.to_string(),
            version: version.to_string(),
            detail: detail.into(),
        }
    }
}

/// Where startup diagnostics go. The runtime forwards them as events.
pub trait PluginDiagnosticSink: Send + Sync {
    fn record(&self, diagnostic: PluginDiagnostic);
}

/// In-memory sink used by diagnostics mode and tests.
#[derive(Debug, Default)]
pub struct MemoryPluginDiagnostics {
    records: Mutex<Vec<PluginDiagnostic>>,
}

impl MemoryPluginDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn records(&self) -> Vec<PluginDiagnostic> {
        self.records.lock().map(|r| r.clone()).unwrap_or_default()
    }

    pub fn failed_count(&self) -> usize {
        self.records()
            .iter()
            .filter(|r| r.kind == "plugin_load_failed")
            .count()
    }
}

impl PluginDiagnosticSink for MemoryPluginDiagnostics {
    fn record(&self, diagnostic: PluginDiagnostic) {
        if let Ok(mut records) = self.records.lock() {
            records.push(diagnostic);
        }
    }
}

/// Outcome of one `load` call, suitable for `GET /v1/runtime/plugins`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadReport {
    pub loaded: Vec<PluginId>,
    pub failed: Vec<PluginId>,
    pub skipped: Vec<PluginId>,
}

impl LoadReport {
    pub fn is_healthy(&self) -> bool {
        self.failed.is_empty() && self.skipped.is_empty()
    }
}

/// One loaded plugin and the handles it owns.
struct PluginRecord {
    manifest: PluginManifest,
    registration: Registration,
}

/// The registry owns every plugin's registration handle, so dropping the
/// registry unregisters all contributions.
pub struct PluginRegistry {
    handles: HostHandles,
    diagnostics: Arc<dyn PluginDiagnosticSink>,
    loaded: BTreeMap<PluginId, PluginRecord>,
}

impl std::fmt::Debug for PluginRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRegistry")
            .field("loaded", &self.loaded.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl PluginRegistry {
    pub fn new(handles: HostHandles, diagnostics: Arc<dyn PluginDiagnosticSink>) -> Self {
        Self {
            handles,
            diagnostics,
            loaded: BTreeMap::new(),
        }
    }

    /// Load every plugin, honouring declaration order and fail-closed skips.
    pub fn load(&mut self, plugins: &[Arc<dyn Plugin>]) -> Result<LoadReport> {
        let manifests: Vec<PluginManifest> = plugins.iter().map(|p| p.manifest().clone()).collect();
        // Declaration errors abort here: nothing has been registered yet.
        let order = resolve_order(&manifests)?;

        let by_id: BTreeMap<PluginId, Arc<dyn Plugin>> = plugins
            .iter()
            .map(|p| (p.manifest().id.clone(), p.clone()))
            .collect();

        let mut report = LoadReport::default();
        let mut unusable: BTreeSet<PluginId> = BTreeSet::new();

        for id in order {
            let Some(plugin) = by_id.get(&id) else {
                continue;
            };
            let manifest = plugin.manifest().clone();
            if let Some(blocker) = first_unusable_dependency(&manifest, &unusable) {
                self.skip(&manifest, &blocker, &mut report, &mut unusable);
                continue;
            }
            match self.load_one(plugin) {
                Ok(record) => {
                    self.diagnostics.record(PluginDiagnostic::new(
                        "plugin_loaded",
                        &manifest.id,
                        &manifest.version.to_string(),
                        format!("kind={}", manifest.kind.as_str()),
                    ));
                    self.loaded.insert(id.clone(), record);
                    report.loaded.push(id);
                }
                Err(err) => {
                    self.diagnostics.record(PluginDiagnostic::new(
                        "plugin_load_failed",
                        &manifest.id,
                        &manifest.version.to_string(),
                        err.message.clone(),
                    ));
                    unusable.insert(id.clone());
                    report.failed.push(id);
                }
            }
        }

        // Dependents of a plugin that never started are skipped, transitively.
        let skipped = plan_skips(
            &manifests
                .into_iter()
                .filter(|m| !report.failed.contains(&m.id))
                .collect::<Vec<_>>(),
            &unusable,
        );
        for plugin_id in &skipped {
            if unusable.contains(plugin_id) {
                continue;
            }
            if let Some(manifest) = by_id.get(plugin_id).map(|p| p.manifest().clone()) {
                if let Some(blocker) = first_unusable_dependency(&manifest, &unusable) {
                    self.skip(&manifest, &blocker, &mut report, &mut unusable);
                }
            }
        }
        Ok(report)
    }

    /// Bind identity and run the plugin's `register`.
    fn load_one(&self, plugin: &Arc<dyn Plugin>) -> Result<PluginRecord> {
        let manifest = plugin.manifest().clone();
        manifest.validate()?;
        let identity = PluginIdentity::host_issue(
            &manifest.id,
            manifest.version.to_string(),
            grant_for(&manifest),
        )?;
        let mut ctx = PluginContext::host_build(identity, &self.handles);
        // Visibility is part of the manifest contract; a bad list is a load
        // failure rather than a silently empty permission set.
        ctx.declare_mcp(&manifest.mcp.expose, &manifest.mcp.consume)?;
        let registration = plugin.register(&mut ctx)?;
        Ok(PluginRecord {
            manifest,
            registration,
        })
    }

    fn skip(
        &self,
        manifest: &PluginManifest,
        blocker: &str,
        report: &mut LoadReport,
        unusable: &mut BTreeSet<PluginId>,
    ) {
        self.diagnostics.record(PluginDiagnostic::new(
            "plugin_skipped",
            &manifest.id,
            &manifest.version.to_string(),
            format!("required service {blocker} is not available"),
        ));
        unusable.insert(manifest.id.clone());
        report.skipped.push(manifest.id.clone());
    }

    /// Loaded plugin ids, in id order.
    pub fn loaded_ids(&self) -> Vec<PluginId> {
        self.loaded.keys().cloned().collect()
    }

    pub fn is_loaded(&self, plugin_id: &str) -> bool {
        self.loaded.contains_key(plugin_id)
    }

    pub fn manifest(&self, plugin_id: &str) -> Option<&PluginManifest> {
        self.loaded.get(plugin_id).map(|record| &record.manifest)
    }

    pub fn registration(&self, plugin_id: &str) -> Option<&Registration> {
        self.loaded
            .get(plugin_id)
            .map(|record| &record.registration)
    }

    /// `/v1/runtime/plugins` payload: enablement plus contributed capabilities.
    pub fn report_json(&self) -> serde_json::Value {
        let plugins: Vec<serde_json::Value> = self
            .loaded
            .values()
            .map(|record| {
                serde_json::json!({
                    "id": record.manifest.id,
                    "version": record.manifest.version.to_string(),
                    "kind": record.manifest.kind.as_str(),
                    "capabilities": record.manifest.capabilities,
                    "pages": record.registration.page_ids(),
                    "panels": record.registration.panel_ids(),
                    "mcp_exposures": record.registration.exposure_ids(),
                })
            })
            .collect();
        serde_json::json!({ "plugins": plugins })
    }

    /// Unload one plugin now: its pages, panels and tools disappear.
    pub fn unload(&mut self, plugin_id: &str) -> bool {
        self.loaded.remove(plugin_id).is_some()
    }

    /// Fail closed when a platform service is absent: the caller skips the
    /// plugin and records a diagnostic instead of running half-wired.
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

/// First dependency of `manifest` that is known-unusable.
fn first_unusable_dependency(
    manifest: &PluginManifest,
    unusable: &BTreeSet<PluginId>,
) -> Option<String> {
    manifest
        .requires
        .keys()
        .find(|service| unusable.contains(*service))
        .cloned()
}

#[cfg(test)]
mod tests;
