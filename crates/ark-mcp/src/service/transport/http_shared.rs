//! Shared HTTP transport settings (design 7.6.1, 7.6.3).
//!
//! Remote MCP servers are addressed exactly like any other plugin egress: the
//! URL must be inside the plugin's allowlist, and the request is issued through
//! the shared capability rather than a private client.

use ark_common::{ArkError, Result};
use url::Url;

/// Endpoint plus identity used on every hop.
#[derive(Debug, Clone)]
pub struct HttpTransportConfig {
    /// Plugin that owns this connection (`ark.example.linear`).
    pub plugin_id: String,
    /// Absolute endpoint URL.
    pub endpoint: String,
    /// Credential reference resolved by the bus at send time.
    pub credential: Option<String>,
    /// Per-request timeout hint in milliseconds.
    pub timeout_ms: Option<u64>,
}

impl HttpTransportConfig {
    pub fn new(plugin_id: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            endpoint: endpoint.into(),
            credential: None,
            timeout_ms: None,
        }
    }

    pub fn with_credential(mut self, reference: impl Into<String>) -> Self {
        self.credential = Some(reference.into());
        self
    }

    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Reject non-HTTP endpoints early so a typo cannot become a runtime error.
    pub fn validate(&self) -> Result<Url> {
        let url = Url::parse(&self.endpoint).map_err(|err| {
            ArkError::validation(format!("invalid mcp endpoint: {err}")).with_detail("invalid_url")
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ArkError::validation(format!(
                "mcp endpoint must be http(s), got {}",
                url.scheme()
            ))
            .with_detail("unsupported_scheme"));
        }
        if url.host_str().is_none() {
            return Err(ArkError::validation("mcp endpoint has no host").with_detail("invalid_url"));
        }
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_and_https_endpoints_are_accepted() {
        assert!(
            HttpTransportConfig::new("ark.a", "https://mcp.example.com/sse")
                .validate()
                .is_ok()
        );
        assert!(
            HttpTransportConfig::new("ark.a", "http://127.0.0.1:9000/mcp")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn non_http_endpoints_are_rejected() {
        for bad in [
            "file:///etc/passwd",
            "stdio://local",
            "not a url",
            "https://",
        ] {
            assert!(
                HttpTransportConfig::new("ark.a", bad).validate().is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn builders_keep_identity_and_options() {
        let config = HttpTransportConfig::new("ark.example.linear", "https://mcp.example.com/mcp")
            .with_credential("token")
            .with_timeout_ms(5_000);
        assert_eq!(config.plugin_id, "ark.example.linear");
        assert_eq!(config.credential.as_deref(), Some("token"));
        assert_eq!(config.timeout_ms, Some(5_000));
    }
}
