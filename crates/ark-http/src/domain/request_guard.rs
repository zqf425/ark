//! Request guards: plugin identity, reserved headers, URL redaction (7.5.5).
//!
//! The plugin supplies only logical values. Everything that identifies the
//! caller (`User-Agent`), everything that addresses the peer (`Host`,
//! `Content-Length`) and everything that authenticates (`Authorization`) is
//! owned by the bus, so a manifest cannot impersonate another plugin.

use ark_common::{ArkError, Result};
use ark_ports::HeaderMap;
use url::Url;

/// Headers a plugin may not set: the bus owns them.
const RESERVED_HEADERS: [&str; 9] = [
    "user-agent",
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "proxy-authorization",
    "authorization",
    "x-plugin-id",
    "x-plugin-version",
];

/// Trace header names mirrored on every outbound hop.
pub const TRACE_HEADER: &str = "x-ark-trace-id";
pub const PLUGIN_HEADER: &str = "x-plugin-id";

pub fn is_reserved_header(name: &str) -> bool {
    RESERVED_HEADERS.contains(&name.trim().to_ascii_lowercase().as_str())
}

/// Validate a plugin id: stable machine identity, no path or space tricks.
pub fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    let ok_len = !plugin_id.is_empty() && plugin_id.len() <= 128;
    let ok_chars = plugin_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
    let ok_shape =
        !plugin_id.starts_with('.') && !plugin_id.ends_with('.') && !plugin_id.contains("..");
    if ok_len && ok_chars && ok_shape {
        Ok(())
    } else {
        Err(ArkError::validation(format!(
            "invalid plugin id: {plugin_id:?}"
        )))
    }
}

/// `User-Agent: ark-plugin/<plugin_id>@<version>` (design 7.5.5).
pub fn user_agent(plugin_id: &str, version: &str) -> String {
    format!("ark-plugin/{plugin_id}@{version}")
}

/// Reject reserved headers instead of silently dropping them: a plugin that
/// tries to set `User-Agent` has a bug we want to surface.
pub fn sanitize_headers(headers: &HeaderMap) -> Result<HeaderMap> {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        let lower = name.trim().to_ascii_lowercase();
        if lower.is_empty() {
            return Err(ArkError::validation("empty header name"));
        }
        if is_reserved_header(&lower) {
            return Err(ArkError::validation(format!(
                "header {lower} is reserved by the capability bus"
            ))
            .with_detail("reserved_header"));
        }
        if value.contains('\r') || value.contains('\n') {
            return Err(ArkError::validation(format!(
                "header {lower} contains a line break"
            )));
        }
        out.insert(lower, value.trim().to_string());
    }
    Ok(out)
}

/// Absolute URL with scheme + host, ready for allowlist matching.
pub fn parse_target(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).map_err(|err| {
        ArkError::validation(format!("invalid url: {err}")).with_detail("invalid_url")
    })?;
    match url.scheme() {
        "https" | "http" => {}
        other => {
            return Err(ArkError::validation(format!("unsupported scheme {other}"))
                .with_detail("unsupported_scheme"))
        }
    }
    if url.host_str().is_none() {
        return Err(ArkError::validation("url has no host").with_detail("invalid_url"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ArkError::validation("url must not carry userinfo").with_detail("invalid_url"));
    }
    Ok(url)
}

/// Lowercased host of a parsed URL.
pub fn host_of(url: &Url) -> String {
    url.host_str().unwrap_or_default().to_ascii_lowercase()
}

/// Strip query values, userinfo and fragments before anything is logged.
pub fn redact_url(url: &Url) -> String {
    let mut clone = url.clone();
    let _ = clone.set_username("");
    let _ = clone.set_password(None);
    clone.set_fragment(None);
    if clone.query().is_some() {
        let keys: Vec<String> = clone
            .query_pairs()
            .map(|(k, _)| format!("{k}=***"))
            .collect();
        clone.set_query(Some(&keys.join("&")));
    }
    clone.to_string()
}

/// Redacted header digest for logs: names only, sensitive values masked.
pub fn header_digest(headers: &HeaderMap) -> Vec<String> {
    headers
        .iter()
        .map(|(name, value)| {
            let lower = name.to_ascii_lowercase();
            if lower.contains("auth") || lower.contains("token") || lower.contains("key") {
                format!("{lower}: ***")
            } else {
                format!("{lower}: {value}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_id_shape_is_enforced() {
        for good in ["ark.builtin.code-ui", "ark.example.jira", "p1", "a_b.c-d"] {
            assert!(validate_plugin_id(good).is_ok(), "{good} should be valid");
        }
        for bad in [
            "",
            "Ark.Builtin",
            "ark..builtin",
            ".ark",
            "ark.",
            "ark/builtin",
            "ark builtin",
        ] {
            assert!(validate_plugin_id(bad).is_err(), "{bad} should be invalid");
        }
    }

    #[test]
    fn reserved_headers_are_denied() {
        let mut headers = HeaderMap::new();
        headers.insert("User-Agent".to_string(), "evil/1.0".to_string());
        let err = sanitize_headers(&headers).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("reserved_header"));
    }

    #[test]
    fn plugins_cannot_forge_identity() {
        for name in [
            "x-plugin-id",
            "X-Plugin-Id",
            "authorization",
            "host",
            "x-plugin-version",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(name.to_string(), "spoof".to_string());
            assert!(
                sanitize_headers(&headers).is_err(),
                "{name} must be reserved"
            );
        }
    }

    #[test]
    fn ordinary_headers_pass_and_are_lowercased() {
        let mut headers = HeaderMap::new();
        headers.insert("Accept".to_string(), "application/json".to_string());
        let out = sanitize_headers(&headers).unwrap();
        assert_eq!(
            out.get("accept").map(String::as_str),
            Some("application/json")
        );
    }

    #[test]
    fn header_injection_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert("x-note".to_string(), "ok\r\nx-evil: 1".to_string());
        assert!(sanitize_headers(&headers).is_err());
    }

    #[test]
    fn userinfo_and_non_http_schemes_are_rejected() {
        assert!(parse_target("https://user:pass@api.example.com/v1").is_err());
        assert!(parse_target("file:///etc/passwd").is_err());
        assert!(parse_target("not a url").is_err());
        assert!(parse_target("https://api.example.com/v1").is_ok());
    }

    #[test]
    fn redaction_hides_query_values() {
        let url = parse_target("https://api.example.com/v1/issue?token=abc123&q=x").unwrap();
        let redacted = redact_url(&url);
        assert!(!redacted.contains("abc123"));
        assert!(redacted.contains("token=***"));
        assert_eq!(host_of(&url), "api.example.com");
    }

    #[test]
    fn user_agent_is_host_owned() {
        assert_eq!(
            user_agent("ark.example.jira", "1.2.3"),
            "ark-plugin/ark.example.jira@1.2.3"
        );
    }
}
