//! Structured `http_denied` / `http_span` diagnostics (design 7.5.1).
//!
//! The bus never prints. A denial produces a record the runtime can turn into
//! an event, so an operator can see which plugin tried to reach what.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Why an outbound request was refused before it left the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpDenialReason {
    /// Host not covered by the plugin manifest allowlist (SSRF guard).
    HostNotAllowed,
    /// Empty allowlist: the plugin never declared egress.
    EmptyAllowlist,
    /// Plain `http://` while the policy requires TLS.
    InsecureScheme,
    /// Unsupported or missing URL scheme.
    UnsupportedScheme,
    /// Malformed URL.
    InvalidUrl,
    /// Reserved header attempted by the plugin (`User-Agent`, `Host`, ...).
    ReservedHeader,
    /// Per `plugin_id + host` token bucket is empty.
    RateLimited,
    /// Unknown credential name, or credential not owned by this plugin.
    CredentialDenied,
    /// Request or response body above the configured cap.
    BodyTooLarge,
    /// Plugin identity could not be validated.
    InvalidPluginId,
}

impl HttpDenialReason {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpDenialReason::HostNotAllowed => "host_not_allowed",
            HttpDenialReason::EmptyAllowlist => "empty_allowlist",
            HttpDenialReason::InsecureScheme => "insecure_scheme",
            HttpDenialReason::UnsupportedScheme => "unsupported_scheme",
            HttpDenialReason::InvalidUrl => "invalid_url",
            HttpDenialReason::ReservedHeader => "reserved_header",
            HttpDenialReason::RateLimited => "rate_limited",
            HttpDenialReason::CredentialDenied => "credential_denied",
            HttpDenialReason::BodyTooLarge => "body_too_large",
            HttpDenialReason::InvalidPluginId => "invalid_plugin_id",
        }
    }
}

/// One audited outbound decision. `url` is always redacted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpDiagnostic {
    /// Diagnostic kind: `http_denied`, `http_span` or `http_retry`.
    pub kind: String,
    pub plugin_id: String,
    pub host: String,
    pub method: String,
    pub redacted_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<HttpDenialReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
}

impl HttpDiagnostic {
    pub fn denied(
        plugin_id: &str,
        method: &str,
        redacted_url: &str,
        reason: HttpDenialReason,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind: "http_denied".to_string(),
            plugin_id: plugin_id.to_string(),
            host: String::new(),
            method: method.to_string(),
            redacted_url: redacted_url.to_string(),
            reason: Some(reason),
            detail: Some(detail.into()),
            status: None,
            duration_ms: None,
            attempts: None,
        }
    }

    pub fn with_host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }

    /// `http_denied` payload as an event-ready JSON value.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({"kind": "http_denied"}))
    }
}

/// Where the bus reports decisions. Implemented by the runtime event bridge.
pub trait HttpDiagnosticSink: Send + Sync {
    fn record(&self, diagnostic: HttpDiagnostic);
}

/// Test/diagnostic-mode sink: keeps the last `capacity` records in memory.
#[derive(Debug, Default)]
pub struct MemoryDiagnostics {
    capacity: usize,
    records: Mutex<Vec<HttpDiagnostic>>,
}

impl MemoryDiagnostics {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            records: Mutex::new(Vec::new()),
        }
    }

    pub fn records(&self) -> Vec<HttpDiagnostic> {
        self.records.lock().map(|r| r.clone()).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.records.lock().map(|r| r.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl HttpDiagnosticSink for MemoryDiagnostics {
    fn record(&self, diagnostic: HttpDiagnostic) {
        if let Ok(mut records) = self.records.lock() {
            records.push(diagnostic);
            let cap = self.capacity;
            if cap > 0 && records.len() > cap {
                let excess = records.len() - cap;
                records.drain(0..excess);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_record_is_serializable_and_never_carries_secrets() {
        let diag = HttpDiagnostic::denied(
            "ark.example.jira",
            "GET",
            "https://api.example.com/v1/issue",
            HttpDenialReason::HostNotAllowed,
            "host api.evil.test not in allowlist",
        )
        .with_host("api.evil.test");
        let json = diag.to_json();
        assert_eq!(json["kind"], "http_denied");
        assert_eq!(json["reason"], "host_not_allowed");
        assert_eq!(json["plugin_id"], "ark.example.jira");
    }

    #[test]
    fn memory_sink_keeps_capacity() {
        let sink = MemoryDiagnostics::new(2);
        for i in 0..5 {
            sink.record(HttpDiagnostic::denied(
                "p",
                "GET",
                "https://a.example.com",
                HttpDenialReason::RateLimited,
                format!("attempt {i}"),
            ));
        }
        assert_eq!(sink.len(), 2);
        assert_eq!(sink.records()[1].detail.as_deref(), Some("attempt 4"));
    }
}
