//! Directory plugin source: `plugins/<plugin_id>/plugin.json` (design 7.7.1).
//!
//! Scanning is deliberately dumb: read one JSON file per directory, validate it
//! with the same rules every other manifest uses, and report what went wrong
//! instead of throwing the whole directory away. Everything that decides *what*
//! a plugin may do lives in [`crate::domain`]; everything that decides *whether*
//! it starts lives in [`crate::domain::isolation`].

use crate::domain::directory::{DirectoryManifest, ToolAction};
use crate::domain::manifest::PluginManifest;
use crate::service::context::PluginContext;
use crate::service::plugin::{Plugin, Registration};
use crate::service::registry::PluginDiagnostic;
use ark_common::{ArkError, Result};
use ark_contract::Capability;
use ark_mcp::{CallContext, McpHandler};
use ark_ports::{HttpCapability, McpCallResult, McpToolSpec, OutboundRequest};
use async_trait::async_trait;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Manifest file name inside one plugin directory.
pub const MANIFEST_FILE: &str = "plugin.json";

/// One directory that produced a valid manifest.
#[derive(Debug, Clone)]
pub struct ScannedPlugin {
    /// Directory the plugin was read from; the plugin's file root.
    pub directory: PathBuf,
    /// Declarations from the file, including contributions.
    pub declarations: DirectoryManifest,
    /// Authoritative manifest derived from those declarations.
    pub manifest: PluginManifest,
}

/// Read every plugin directory under `root`.
///
/// A missing root is "no plugins installed", not an error: a fresh data
/// directory must start the runtime. Every rejected directory yields a
/// diagnostic so an operator can see what was skipped and why.
pub fn scan(root: &Path) -> (Vec<ScannedPlugin>, Vec<PluginDiagnostic>) {
    let mut scanned = Vec::new();
    let mut diagnostics = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return (scanned, diagnostics);
    };
    let mut directories: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|entry| entry.path())
        .collect();
    // Deterministic order keeps startup logs and diagnostics comparable.
    directories.sort();
    for directory in directories {
        match read_one(&directory) {
            Ok(plugin) => scanned.push(plugin),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    (scanned, diagnostics)
}

/// Read and validate one plugin directory.
fn read_one(directory: &Path) -> std::result::Result<ScannedPlugin, PluginDiagnostic> {
    let name = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = directory.join(MANIFEST_FILE);
    let raw = std::fs::read_to_string(&path)
        .map_err(|err| invalid(&name, "", format!("cannot read {}: {err}", path.display())))?;
    let declarations: DirectoryManifest = serde_json::from_str(&raw).map_err(|err| {
        invalid(
            &name,
            "",
            format!("{} is not valid JSON: {err}", path.display()),
        )
    })?;
    if declarations.id != name {
        // The directory name is the plugin's file root, so a mismatch would let
        // one plugin read another's files.
        return Err(invalid(
            &name,
            &declarations.version,
            format!(
                "manifest id {} does not match directory {name}",
                declarations.id
            ),
        ));
    }
    let manifest = declarations
        .to_manifest()
        .map_err(|err| invalid(&name, &declarations.version, err.message.clone()))?;
    Ok(ScannedPlugin {
        directory: directory.to_path_buf(),
        declarations,
        manifest,
    })
}

fn invalid(plugin_id: &str, version: &str, detail: impl Into<String>) -> PluginDiagnostic {
    PluginDiagnostic::new("plugin_manifest_invalid", plugin_id, version, detail)
}

/// A plugin whose contributions are declarations rather than code.
///
/// It implements the same [`Plugin`] contract as a compiled plugin, so the
/// loader, the isolation rules and `Registration` drop semantics are identical.
pub struct DirectoryPlugin {
    manifest: PluginManifest,
    declarations: DirectoryManifest,
    http: Arc<dyn HttpCapability>,
}

impl std::fmt::Debug for DirectoryPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryPlugin")
            .field("id", &self.manifest.id)
            .field("version", &self.manifest.version.to_string())
            .finish_non_exhaustive()
    }
}

impl DirectoryPlugin {
    pub fn new(scanned: &ScannedPlugin, http: Arc<dyn HttpCapability>) -> Self {
        Self {
            manifest: scanned.manifest.clone(),
            declarations: scanned.declarations.clone(),
            http,
        }
    }

    /// The declarations behind this plugin, for the host report.
    pub fn declarations(&self) -> &DirectoryManifest {
        &self.declarations
    }

    /// Reject a file that promises one thing and ships another.
    ///
    /// Checked before any registration, so a mismatch never leaves a half
    /// registered plugin behind (design 7.7.3).
    fn check_declaration(&self) -> Result<()> {
        let declared: BTreeSet<&str> = self
            .manifest
            .mcp
            .expose
            .iter()
            .map(String::as_str)
            .collect();
        let implemented: BTreeSet<&str> = self
            .declarations
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        if declared != implemented {
            return Err(mismatch(format!(
                "mcp.expose {:?} does not match tools {:?}",
                declared, implemented
            )));
        }
        self.check_capability(
            !self.declarations.ui.pages.is_empty(),
            Capability::UiPage,
            "ui.pages",
        )?;
        self.check_capability(
            !self.declarations.ui.panels.is_empty(),
            Capability::UiPanel,
            "ui.panels",
        )?;
        self.check_capability(
            !self.declarations.tools.is_empty(),
            Capability::Tool,
            "tools",
        )?;
        Ok(())
    }

    /// A contribution needs the capability that authorises it.
    fn check_capability(&self, present: bool, capability: Capability, section: &str) -> Result<()> {
        if present && !self.manifest.declares(capability) {
            return Err(mismatch(format!(
                "{section} is declared but capability {capability:?} is not"
            )));
        }
        Ok(())
    }

    /// Build one handler per declared tool, bound to this plugin's id.
    fn handlers(&self) -> Vec<(McpToolSpec, Arc<dyn McpHandler>)> {
        let plugin_id = self.manifest.id.clone();
        self.declarations
            .tools
            .iter()
            .map(|tool| {
                let handler: Arc<dyn McpHandler> = match &tool.action {
                    ToolAction::Static { content } => Arc::new(StaticHandler {
                        content: content.clone(),
                    }),
                    ToolAction::Http {
                        method,
                        url,
                        body,
                        headers,
                        credential,
                    } => Arc::new(HttpHandler {
                        plugin_id: plugin_id.clone(),
                        request: OutboundRequest {
                            method: method.clone(),
                            url: url.clone(),
                            headers: headers
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                            body: body.clone(),
                            timeout_ms: None,
                            idempotency_key: None,
                            credential: credential.clone(),
                        },
                        http: Arc::clone(&self.http),
                    }),
                };
                (tool.spec(&plugin_id), handler)
            })
            .collect()
    }
}

fn mismatch(detail: impl Into<String>) -> ArkError {
    ArkError::validation(detail).with_detail("plugin_declaration_mismatch")
}

impl Plugin for DirectoryPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn register(&self, ctx: &mut PluginContext) -> Result<Registration> {
        self.check_declaration()?;
        let mut registration = Registration::new();
        for page in &self.declarations.ui.pages {
            registration.push_page(ctx.register_page(page.spec())?);
        }
        for panel in &self.declarations.ui.panels {
            registration.push_panel(ctx.register_panel(panel.slot, panel.spec())?);
        }
        if !self.declarations.tools.is_empty() {
            registration.push_exposure(ctx.expose_tool_handlers(self.handlers())?);
        }
        Ok(registration)
    }
}

/// Returns the declared payload unchanged.
struct StaticHandler {
    content: serde_json::Value,
}

#[async_trait]
impl McpHandler for StaticHandler {
    async fn call(&self, _ctx: &CallContext, _args: serde_json::Value) -> Result<McpCallResult> {
        Ok(McpCallResult {
            ok: true,
            content: self.content.clone(),
            trace_id: String::new(),
        })
    }
}

/// One governed outbound request on behalf of the owning plugin.
///
/// The bus applies the allowlist, header guards, quota and credential
/// injection; this handler only carries the declaration.
struct HttpHandler {
    plugin_id: String,
    request: OutboundRequest,
    http: Arc<dyn HttpCapability>,
}

#[async_trait]
impl McpHandler for HttpHandler {
    async fn call(&self, _ctx: &CallContext, _args: serde_json::Value) -> Result<McpCallResult> {
        let response = self
            .http
            .request(&self.plugin_id, self.request.clone())
            .await?;
        Ok(McpCallResult {
            ok: response.status < 400,
            content: serde_json::json!({ "status": response.status, "body": response.body }),
            trace_id: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PluginKind;
    use std::sync::Arc;

    fn write_plugin(root: &Path, name: &str, body: serde_json::Value) {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    fn hello() -> serde_json::Value {
        serde_json::json!({
            "id": "ark.example.hello",
            "version": "1.0.0",
            "kind": "tool",
            "capabilities": ["tool", "ui_panel"],
            "mcp": { "expose": ["greet"] },
            "ui": { "panels": [{ "id": "ark.example.hello.panel", "slot": "right", "title": "Hello" }] },
            "tools": [{ "name": "greet", "description": "says hello", "action": { "kind": "static", "content": { "text": "hi" } } }]
        })
    }

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ark-plugin-scan-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_missing_root_is_no_plugins_not_an_error() {
        let root = temp_root("missing").join("absent");
        let (scanned, diagnostics) = scan(&root);
        assert!(scanned.is_empty());
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn a_valid_directory_is_scanned() {
        let root = temp_root("valid");
        write_plugin(&root, "ark.example.hello", hello());
        let (scanned, diagnostics) = scan(&root);
        assert_eq!(scanned.len(), 1);
        assert!(diagnostics.is_empty());
        assert_eq!(scanned[0].manifest.id, "ark.example.hello");
        assert_eq!(scanned[0].declarations.tools.len(), 1);
    }

    #[test]
    fn broken_json_is_a_diagnostic_and_healthy_plugins_survive() {
        let root = temp_root("broken");
        write_plugin(&root, "ark.example.hello", hello());
        let broken = root.join("ark.example.broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(MANIFEST_FILE), b"{ not json").unwrap();

        let (scanned, diagnostics) = scan(&root);
        assert_eq!(scanned.len(), 1);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, "plugin_manifest_invalid");
        assert_eq!(diagnostics[0].plugin_id, "ark.example.broken");
    }

    #[test]
    fn a_directory_name_that_disagrees_with_the_manifest_is_rejected() {
        let root = temp_root("mismatch-name");
        write_plugin(&root, "ark.example.other", hello());
        let (scanned, diagnostics) = scan(&root);
        assert!(scanned.is_empty());
        assert!(diagnostics[0].detail.contains("does not match directory"));
    }

    #[test]
    fn a_manifest_that_promises_an_unimplemented_tool_is_rejected_at_register() {
        let root = temp_root("declaration");
        let mut body = hello();
        body["mcp"] = serde_json::json!({ "expose": ["greet", "absent"] });
        write_plugin(&root, "ark.example.hello", body);
        let (scanned, _) = scan(&root);
        let plugin = DirectoryPlugin::new(&scanned[0], test_http());
        let err = plugin.check_declaration().unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_declaration_mismatch"));
    }

    #[test]
    fn a_contribution_without_its_capability_is_rejected() {
        let root = temp_root("capability");
        let mut body = hello();
        body["capabilities"] = serde_json::json!(["tool"]);
        write_plugin(&root, "ark.example.hello", body);
        let (scanned, _) = scan(&root);
        let plugin = DirectoryPlugin::new(&scanned[0], test_http());
        assert!(plugin.check_declaration().is_err());
    }

    fn test_http() -> Arc<dyn HttpCapability> {
        Arc::new(
            ark_http::HttpBus::new(
                ark_http::HttpPolicy::default(),
                Arc::new(ark_http::CredentialVault::new()),
                Arc::new(ark_http::MemoryDiagnostics::new(16)),
            )
            .unwrap(),
        )
    }

    #[test]
    fn a_manifest_kind_round_trips_for_the_report() {
        let root = temp_root("kind");
        write_plugin(&root, "ark.example.hello", hello());
        let (scanned, _) = scan(&root);
        assert_eq!(scanned[0].manifest.kind, PluginKind::Tool);
    }
}
