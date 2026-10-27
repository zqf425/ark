//! External MCP server entry rules: identity, command and endpoint checks.
//!
//! Validation lives here rather than in the registry so the HTTP route, the
//! registry and the tests all reject the same input with the same message. Every
//! message is phrased to be shown to the user as-is (design 7.6.1).

use ark_common::{ArkError, Result};
use ark_contract::{McpServerEntry, McpTransportKind};
use std::collections::BTreeMap;
use url::Url;

/// Longest accepted server id; matches the contract `^[A-Za-z0-9._-]{1,64}$`.
pub const MAX_ID_LEN: usize = 64;

/// Reject an id that could not be persisted or addressed by a route.
///
/// The charset is deliberately narrower than the HTTP layer's plugin ids: a
/// server id also becomes a tool namespace (`<server>.<tool>`), so spaces, `*`
/// and `:` would break the qualified name assembly later.
pub fn validate_id(id: &str) -> Result<()> {
    let id = id.trim();
    if id.is_empty() {
        return Err(
            ArkError::validation("mcp server id must not be empty").with_detail("invalid_id")
        );
    }
    if id.len() > MAX_ID_LEN {
        return Err(ArkError::validation(format!(
            "mcp server id must be at most {MAX_ID_LEN} characters"
        ))
        .with_detail("invalid_id"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(ArkError::validation(format!(
            "mcp server id {id:?} may only contain letters, digits, '.', '-' and '_'"
        ))
        .with_detail("invalid_id"));
    }
    Ok(())
}

/// Parse and check the endpoint of an `sse` / `streamable_http` server.
pub fn endpoint_url(entry: &McpServerEntry) -> Result<Url> {
    let raw = entry
        .url
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .ok_or_else(|| {
            ArkError::validation(format!(
                "mcp server {} uses {} and requires a url",
                entry.id,
                entry.transport.as_str()
            ))
            .with_detail("invalid_url")
        })?;
    let url = Url::parse(raw).map_err(|err| {
        ArkError::validation(format!("mcp server {} has an invalid url: {err}", entry.id))
            .with_detail("invalid_url")
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ArkError::validation(format!(
            "mcp server {} url must use http or https, got {}",
            entry.id,
            url.scheme()
        ))
        .with_detail("unsupported_scheme"));
    }
    if url.host_str().is_none() {
        return Err(
            ArkError::validation(format!("mcp server {} url has no host", entry.id))
                .with_detail("invalid_url"),
        );
    }
    Ok(url)
}

/// Validate one entry exactly as it will be persisted.
pub fn validate(entry: &McpServerEntry) -> Result<()> {
    validate_id(&entry.id)?;
    match entry.transport {
        McpTransportKind::Stdio => {
            let command = entry.command.as_deref().map(str::trim).unwrap_or_default();
            if command.is_empty() {
                return Err(ArkError::validation(format!(
                    "mcp server {} uses the stdio transport and requires a command",
                    entry.id
                ))
                .with_detail("invalid_command"));
            }
        }
        McpTransportKind::StreamableHttp | McpTransportKind::Sse => {
            endpoint_url(entry)?;
        }
    }
    Ok(())
}

/// Trim user input, drop empties, then validate.
///
/// Normalising before comparing ids keeps `" jira "` and `"jira"` from becoming
/// two servers that look identical in the GUI.
pub fn normalize(mut entry: McpServerEntry) -> Result<McpServerEntry> {
    entry.id = entry.id.trim().to_string();
    entry.command = non_empty(entry.command);
    entry.url = non_empty(entry.url);
    entry.args = entry
        .args
        .into_iter()
        .map(|arg| arg.trim().to_string())
        .filter(|arg| !arg.is_empty())
        .collect();
    entry.env = clean_map(entry.env);
    entry.headers = clean_map(entry.headers);
    entry.allow_tools = entry
        .allow_tools
        .into_iter()
        .map(|tool| tool.trim().to_string())
        .filter(|tool| !tool.is_empty())
        .collect();
    validate(&entry)?;
    Ok(entry)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn clean_map(map: BTreeMap<String, String>) -> BTreeMap<String, String> {
    map.into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim().to_string();
            if key.is_empty() {
                return None;
            }
            Some((key, value.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(id: &str, command: Option<&str>) -> McpServerEntry {
        McpServerEntry {
            id: id.to_string(),
            transport: McpTransportKind::Stdio,
            enabled: true,
            command: command.map(str::to_string),
            args: Vec::new(),
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
            allow_tools: Vec::new(),
        }
    }

    #[test]
    fn id_charset_and_length_are_enforced() {
        for good in ["jira", "my-server", "a.b_c9", &"x".repeat(MAX_ID_LEN)] {
            assert!(validate_id(good).is_ok(), "{good} should be accepted");
        }
        for bad in [
            "",
            "   ",
            "has space",
            "star*",
            "colon:x",
            "sla/sh",
            &"y".repeat(65),
        ] {
            let err = validate_id(bad).unwrap_err();
            assert_eq!(err.code, ark_common::ArkErrorCode::ValidationError);
            assert_eq!(err.detail.as_deref(), Some("invalid_id"));
        }
    }

    #[test]
    fn stdio_requires_a_command() {
        assert!(validate(&stdio("local", Some("npx"))).is_ok());
        for missing in [None, Some("   ")] {
            let err = validate(&stdio("local", missing)).unwrap_err();
            assert_eq!(err.detail.as_deref(), Some("invalid_command"));
            assert!(err.message.contains("requires a command"));
        }
    }

    #[test]
    fn http_transports_require_a_usable_url() {
        let mut entry = stdio("remote", None);
        entry.transport = McpTransportKind::StreamableHttp;
        let err = validate(&entry).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("invalid_url"));

        for bad in [
            "not a url",
            "file:///etc/passwd",
            "ftp://example.com/mcp",
            "https://",
        ] {
            entry.url = Some(bad.to_string());
            assert!(validate(&entry).is_err(), "{bad} should be rejected");
        }

        for good in ["https://mcp.example.com/mcp", "http://127.0.0.1:9000/mcp"] {
            entry.url = Some(good.to_string());
            assert!(validate(&entry).is_ok(), "{good} should be accepted");
        }
    }

    #[test]
    fn normalize_trims_and_drops_blank_fields() {
        let mut entry = stdio("  local  ", Some("  npx  "));
        entry.args = vec!["  -y ".to_string(), "   ".to_string()];
        entry
            .env
            .insert("  TOKEN ".to_string(), "  abc  ".to_string());
        entry.env.insert("   ".to_string(), "ignored".to_string());
        let entry = normalize(entry).unwrap();
        assert_eq!(entry.id, "local");
        assert_eq!(entry.command.as_deref(), Some("npx"));
        assert_eq!(entry.args, vec!["-y".to_string()]);
        assert_eq!(entry.env.get("TOKEN").map(String::as_str), Some("abc"));
        assert_eq!(entry.env.len(), 1);
    }

    #[test]
    fn normalize_still_validates() {
        assert!(normalize(stdio("bad id", Some("npx"))).is_err());
        assert!(normalize(stdio("ok", None)).is_err());
    }
}
