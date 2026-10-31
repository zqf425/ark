//! The three base capabilities of the bus (design §7.5).
//!
//! Plugins call these instead of building their own HTTP client, MCP server or
//! window. That is what keeps proxy/TLS/retry/credential policy in one place.

use ark_common::Result;
use ark_contract::{Capability, PageSpec, PanelSlot, PanelSpec, UiCatalog};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub type HeaderMap = std::collections::BTreeMap<String, String>;

/// Outbound request. The URL must fall inside the plugin's manifest allowlist,
/// otherwise the call is denied and audited (`http_denied`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutboundRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: HeaderMap,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Logical credential name; the bus injects the secret, the plugin never
    /// sees the plaintext.
    #[serde(default)]
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutboundResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: String,
}

#[async_trait]
pub trait HttpCapability: Send + Sync {
    async fn request(&self, plugin_id: &str, req: OutboundRequest) -> Result<OutboundResponse>;
}

/// A tool a plugin exposes through the single MCP egress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolSpec {
    pub plugin_id: String,
    pub tool: String,
    pub description: String,
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCallResult {
    pub ok: bool,
    pub content: serde_json::Value,
    #[serde(default)]
    pub trace_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpCatalog {
    pub tools: Vec<McpToolSpec>,
}

/// Owns the single MCP egress. Tool names are always `<plugin_id>.<tool>`.
#[async_trait]
pub trait McpCapability: Send + Sync {
    fn expose(&self, tools: Vec<McpToolSpec>) -> Result<()>;

    async fn call(
        &self,
        caller: &str,
        provider: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<McpCallResult>;

    fn catalog(&self) -> McpCatalog;
}

/// Opaque page id returned by a port-level registration.
///
/// It carries no lifetime behaviour: the port trait has no `Drop`, so a caller
/// that needs deregistration must use the implementation's scoped guard
/// (`UiBus::register_page`) instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageHandle {
    pub id: String,
}

/// Opaque panel id; see [`PageHandle`] for the ownership caveat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelHandle {
    pub id: String,
}

/// Owns routing, theme, auth and the event bridge for every page.
///
/// The catalog served by `GET /v1/runtime/capabilities` comes from here, so the
/// HTTP layer never keeps a second navigation list of its own.
#[async_trait]
pub trait UiCapability: Send + Sync {
    fn register_page(&self, page: PageSpec) -> Result<PageHandle>;

    fn register_panel(&self, slot: PanelSlot, panel: PanelSpec) -> Result<PanelHandle>;

    fn pages(&self) -> Vec<PageSpec>;

    /// Capabilities the UI bus itself provides.
    ///
    /// `ui_page`/`ui_panel` follow the registrations, so the host does not have
    /// to keep a parallel grant list. Capabilities owned by other ports (tools,
    /// model providers) are composed by the HTTP layer from their own services.
    fn capabilities(&self) -> Vec<Capability>;

    /// The page/panel catalog, filtered by the caller's full capability set.
    ///
    /// Takes the complete set because a page may require a capability this bus
    /// does not own (for example `tool`); filtering against only the bus's own
    /// grants would hide pages the runtime can actually serve.
    fn catalog(&self, available: &[Capability]) -> UiCatalog;
}
