//! Local plugin directory format: `plugins/<plugin_id>/plugin.json`.
//!
//! This is a *source format*, not a second rule set. It carries the manifest
//! fields a hand-written file needs in a human-friendly spelling (a version
//! string instead of the serialized `Version` object) plus the declarative
//! contributions the plugin makes. Validation is still
//! [`PluginManifest::validate`], so a directory plugin cannot be laxer than any
//! other plugin.
//!
//! ## Why contributions are declarative
//!
//! The first host version has no dynamic code loading (no dylib, no WASM), so a
//! directory plugin declares what it publishes: pages, panels and MCP tools.
//! Each tool names one *host-provided* action (`static` or governed `http`).
//! The plugin therefore never gets a way to run arbitrary code, and every
//! capability still travels through the same scoped handles a compiled plugin
//! uses.

use ark_contract::{Capability, PageId, PageSpec, PanelSlot, PanelSpec, Placement};
use ark_ports::McpToolSpec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::domain::manifest::{HttpSection, McpSection, PluginKind, PluginManifest, Version};

/// `plugin.json` as written on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryManifest {
    pub id: String,
    /// Version string, e.g. `1.2.3`; parsed with [`Version::parse`].
    pub version: String,
    pub kind: PluginKind,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub requires: BTreeMap<String, String>,
    #[serde(default)]
    pub http: HttpSection,
    #[serde(default)]
    pub mcp: McpSection,
    #[serde(default)]
    pub ui: UiSection,
    #[serde(default)]
    pub tools: Vec<ToolDecl>,
}

impl DirectoryManifest {
    /// Build the authoritative manifest. Every rule stays in [`PluginManifest`].
    pub fn to_manifest(&self) -> ark_common::Result<PluginManifest> {
        let manifest = PluginManifest {
            id: self.id.clone(),
            version: Version::parse(&self.version)?,
            kind: self.kind,
            capabilities: self.capabilities.clone(),
            config_schema: self.config_schema.clone(),
            requires: self.requires.clone(),
            http: self.http.clone(),
            mcp: self.mcp.clone(),
        };
        manifest.validate()?;
        Ok(manifest)
    }
}

/// `ui` section: pages and panels the plugin publishes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSection {
    #[serde(default)]
    pub pages: Vec<PageDecl>,
    #[serde(default)]
    pub panels: Vec<PanelDecl>,
}

/// One navigation page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageDecl {
    pub id: String,
    pub route: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default = "default_placement")]
    pub placement: Placement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
    #[serde(default)]
    pub required_caps: Vec<Capability>,
}

impl PageDecl {
    pub fn spec(&self) -> PageSpec {
        PageSpec {
            id: PageId::new(&self.id),
            route: self.route.clone(),
            title: self.title.clone(),
            icon: self.icon.clone(),
            placement: self.placement,
            bundle: self.bundle.clone(),
            required_caps: self.required_caps.clone(),
        }
    }
}

/// One embeddable panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelDecl {
    pub id: String,
    pub slot: PanelSlot,
    pub title: String,
    #[serde(default)]
    pub required_caps: Vec<Capability>,
}

impl PanelDecl {
    pub fn spec(&self) -> PanelSpec {
        PanelSpec {
            id: PageId::new(&self.id),
            slot: self.slot,
            title: self.title.clone(),
            required_caps: self.required_caps.clone(),
        }
    }
}

/// One published MCP tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDecl {
    /// Local tool name; the bus qualifies it as `<plugin_id>.<name>`.
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    #[serde(default)]
    pub action: ToolAction,
}

impl ToolDecl {
    /// Spec the bus publishes. `plugin_id` is filled in by the loader, never by
    /// the file, so a manifest cannot claim another plugin's namespace.
    pub fn spec(&self, plugin_id: &str) -> McpToolSpec {
        McpToolSpec {
            plugin_id: plugin_id.to_string(),
            tool: self.name.clone(),
            description: self.description.clone(),
            schema: self
                .schema
                .clone()
                .filter(serde_json::Value::is_object)
                .unwrap_or_else(empty_schema),
        }
    }
}

/// What a declared tool does when called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolAction {
    /// Returns the declared payload unchanged.
    Static {
        #[serde(default)]
        content: serde_json::Value,
    },
    /// One governed outbound request through `HttpCapability`.
    Http {
        #[serde(default = "default_method")]
        method: String,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        /// Logical credential name; the bus injects the secret.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential: Option<String>,
    },
}

fn default_placement() -> Placement {
    Placement::PrimaryNav
}

/// A tool without an explicit action only returns its payload.
impl Default for ToolAction {
    fn default() -> Self {
        ToolAction::Static {
            content: serde_json::Value::Null,
        }
    }
}

fn default_method() -> String {
    "GET".to_string()
}

/// Minimal object schema for a tool that declares none.
fn empty_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> serde_json::Value {
        serde_json::json!({
            "id": "ark.example.hello",
            "version": "1.0.0",
            "kind": "tool",
            "capabilities": ["tool", "ui_panel"],
            "mcp": { "expose": ["greet"] },
            "ui": {
                "panels": [{ "id": "ark.example.hello.panel", "slot": "right", "title": "Hello" }]
            },
            "tools": [{ "name": "greet", "description": "says hello", "action": { "kind": "static", "content": { "text": "hi" } } }]
        })
    }

    #[test]
    fn a_directory_manifest_becomes_a_validated_manifest() {
        let raw: DirectoryManifest = serde_json::from_value(minimal()).unwrap();
        let manifest = raw.to_manifest().unwrap();
        assert_eq!(manifest.id, "ark.example.hello");
        assert_eq!(manifest.version.to_string(), "1.0.0");
        assert_eq!(manifest.mcp.expose, vec!["greet".to_string()]);
        assert_eq!(raw.ui.panels.len(), 1);
    }

    #[test]
    fn an_invalid_version_is_rejected_before_any_registration() {
        let mut value = minimal();
        value["version"] = serde_json::json!("1");
        let raw: DirectoryManifest = serde_json::from_value(value).unwrap();
        assert!(raw.to_manifest().is_err());
    }

    #[test]
    fn a_bad_allowlist_entry_fails_manifest_validation() {
        let mut value = minimal();
        value["http"] = serde_json::json!({ "allow": ["https://api.example.com"] });
        let raw: DirectoryManifest = serde_json::from_value(value).unwrap();
        assert!(raw.to_manifest().is_err());
    }

    #[test]
    fn tool_specs_are_qualified_by_the_loader_not_the_file() {
        let raw: DirectoryManifest = serde_json::from_value(minimal()).unwrap();
        let spec = raw.tools[0].spec("ark.example.hello");
        assert_eq!(spec.plugin_id, "ark.example.hello");
        assert_eq!(spec.tool, "greet");
        assert!(spec.schema.is_object());
    }

    #[test]
    fn an_http_action_defaults_to_get() {
        let raw: DirectoryManifest = serde_json::from_value(serde_json::json!({
            "id": "ark.example.web",
            "version": "1.0.0",
            "kind": "tool",
            "tools": [{ "name": "ping", "action": { "kind": "http", "url": "https://api.example.com/ping" } }]
        }))
        .unwrap();
        match &raw.tools[0].action {
            ToolAction::Http { method, .. } => assert_eq!(method, "GET"),
            other => panic!("expected http action, got {other:?}"),
        }
    }
}
