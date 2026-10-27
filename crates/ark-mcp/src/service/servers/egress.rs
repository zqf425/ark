//! HTTP egress for `sse` / `streamable_http` MCP servers.
//!
//! The MCP transports in [`crate::service::transport`] never build their own
//! client: they ask an [`HttpCapability`] to send the request. This module
//! supplies that capability for user-configured external servers by delegating
//! to [`ark_http::HttpBus`], so proxy, TLS, redirect and body-cap policy stay in
//! one place instead of a second `reqwest::Client` inside this crate.
//!
//! ## Per-server identity
//!
//! `HttpBus` authorises by plugin id plus host allowlist, so every server entry
//! is registered under a derived id (`mcp.<server>`) whose allowlist contains
//! only the host of its own URL. Re-registering after an edit replaces the old
//! grant, which is what the upsert path needs.
//!
//! ## User-supplied headers
//!
//! `entry.headers` is user configuration, not plugin configuration:
//!
//! * `authorization` is stored in the credential vault and injected by the bus,
//!   because that header name is reserved by the capability surface;
//! * other reserved headers (`host`, `user-agent`, `content-length`, ...) are
//!   ignored because the bus owns them;
//! * everything else is merged into each outbound request.

use ark_common::{ArkError, Result};
use ark_contract::McpServerEntry;
use ark_http::{is_reserved_header, HttpBus};
use ark_ports::{HeaderMap, HttpCapability, OutboundRequest, OutboundResponse};
use async_trait::async_trait;
use std::sync::Arc;

use super::super::transport::HttpTransportConfig;
use crate::domain::server_entry::endpoint_url;

/// Credential name used for a configured `authorization` header.
const AUTH_CREDENTIAL: &str = "authorization";

/// Identity + header policy for one remote server.
pub(crate) struct ServerEgress {
    bus: Arc<HttpBus>,
    plugin_id: String,
    headers: HeaderMap,
    credential: Option<String>,
}

impl std::fmt::Debug for ServerEgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerEgress")
            .field("plugin_id", &self.plugin_id)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("credential", &self.credential.is_some())
            .finish_non_exhaustive()
    }
}

impl ServerEgress {
    /// Register the server's egress grant and split its headers.
    pub(crate) fn new(bus: Arc<HttpBus>, entry: &McpServerEntry) -> Result<Arc<Self>> {
        let url = endpoint_url(entry)?;
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let plugin_id = egress_plugin_id(&entry.id);
        bus.register_plugin(&plugin_id, env!("CARGO_PKG_VERSION"), &[host])?;

        let mut headers = HeaderMap::new();
        let mut credential = None;
        for (name, value) in &entry.headers {
            let lower = name.trim().to_ascii_lowercase();
            if lower.is_empty() {
                continue;
            }
            if value.contains('\r') || value.contains('\n') {
                return Err(ArkError::validation(format!(
                    "mcp server {} header {lower} contains a line break",
                    entry.id
                ))
                .with_detail("invalid_header"));
            }
            if lower == AUTH_CREDENTIAL {
                if let Some(secret) = strip_bearer(value.trim()) {
                    store_secret(&bus, &plugin_id, secret)?;
                    credential = Some(AUTH_CREDENTIAL.to_string());
                }
                continue;
            }
            if is_reserved_header(&lower) {
                continue;
            }
            headers.insert(lower, value.trim().to_string());
        }

        Ok(Arc::new(Self {
            bus,
            plugin_id,
            headers,
            credential,
        }))
    }

    pub(crate) fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Transport config pointing at `entry.url`.
    pub(crate) fn config(entry: &McpServerEntry, plugin_id: &str) -> Result<HttpTransportConfig> {
        let url = endpoint_url(entry)?;
        Ok(HttpTransportConfig::new(plugin_id, url.to_string())
            .with_timeout_ms(super::CONNECT_TIMEOUT_MS))
    }
}

#[async_trait]
impl HttpCapability for ServerEgress {
    async fn request(
        &self,
        _plugin_id: &str,
        mut req: OutboundRequest,
    ) -> Result<OutboundResponse> {
        for (name, value) in &self.headers {
            req.headers.insert(name.clone(), value.clone());
        }
        if self.credential.is_some() {
            req.credential = self.credential.clone();
        }
        self.bus.request(&self.plugin_id, req).await
    }
}

/// Derive a valid bus plugin id from a server id.
///
/// Server ids may contain uppercase letters and repeated dots, neither of which
/// the bus accepts, so the derived id is lowercased and collapsed. Two server
/// ids that differ only in case share one grant; the registry normalises ids to
/// lowercase on write, so that collision cannot be created through the API.
pub(crate) fn egress_plugin_id(server_id: &str) -> String {
    let mut clean = String::new();
    for ch in server_id.trim().to_ascii_lowercase().chars() {
        if ch == '.' {
            if !clean.is_empty() && !clean.ends_with('.') {
                clean.push('.');
            }
        } else if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            clean.push(ch);
        }
    }
    let clean = clean.trim_end_matches('.').to_string();
    if clean.is_empty() {
        "mcp.server".to_string()
    } else {
        format!("mcp.{clean}")
    }
}

fn store_secret(bus: &Arc<HttpBus>, plugin_id: &str, secret: &str) -> Result<()> {
    bus.vault()
        .store_for(plugin_id, AUTH_CREDENTIAL, secret)
        .map_err(|err| {
            ArkError::validation(format!(
                "invalid authorization value for {plugin_id}: {}",
                err.message
            ))
        })
}

/// Accept both `Bearer <token>` and a bare token.
fn strip_bearer(value: &str) -> Option<&str> {
    if value.is_empty() {
        return None;
    }
    if value.len() > 7 && value[..7].eq_ignore_ascii_case("bearer ") {
        let rest = value[7..].trim();
        return if rest.is_empty() { None } else { Some(rest) };
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_ids_map_to_valid_bus_plugin_ids() {
        assert_eq!(egress_plugin_id("jira"), "mcp.jira");
        assert_eq!(egress_plugin_id("  My-Server  "), "mcp.my-server");
        assert_eq!(egress_plugin_id(".x"), "mcp.x");
        assert_eq!(egress_plugin_id(".."), "mcp.server");
        for id in ["jira", "My.Server_1", "..weird..", ""] {
            let derived = egress_plugin_id(id);
            assert!(
                ark_http::validate_plugin_id(&derived).is_ok(),
                "{derived} must be valid"
            );
        }
    }

    #[test]
    fn bearer_prefix_is_stripped_case_insensitively() {
        assert_eq!(strip_bearer("Bearer abc"), Some("abc"));
        assert_eq!(strip_bearer("bearer   abc "), Some("abc"));
        assert_eq!(strip_bearer("abc"), Some("abc"));
        assert_eq!(strip_bearer(""), None);
        assert_eq!(strip_bearer("Bearer   "), None);
    }
}
