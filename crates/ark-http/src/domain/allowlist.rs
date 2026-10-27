//! Manifest host allowlist matching (design 7.5.1).
//!
//! Pure string rules: a plugin may only reach hosts it declared in
//! `manifest.http.allow`. Anything else is denied and audited.

use ark_common::{ArkError, Result};

/// One allowlist entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPattern {
    /// `api.example.com`
    Exact(String),
    /// `*.corp.example`: subdomains only, never the bare apex.
    Subdomain(String),
}

impl HostPattern {
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ArkError::validation("allowlist entry is empty"));
        }
        if raw.contains("://") || raw.contains('/') || raw.contains('?') || raw.contains('#') {
            return Err(ArkError::validation(format!(
                "allowlist entry must be a bare host, got {raw}"
            )));
        }
        if raw.contains('@') || raw.contains(':') {
            return Err(ArkError::validation(format!(
                "allowlist entry must not carry userinfo or a port, got {raw}"
            )));
        }
        let lower = raw.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("*.") {
            if rest.is_empty() || rest.contains('*') {
                return Err(ArkError::validation(format!(
                    "invalid wildcard entry {raw}"
                )));
            }
            return Ok(HostPattern::Subdomain(rest.to_string()));
        }
        if lower.contains('*') {
            return Err(ArkError::validation(format!(
                "wildcard is only allowed as a leading label, got {raw}"
            )));
        }
        Ok(HostPattern::Exact(lower))
    }

    pub fn matches(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        match self {
            HostPattern::Exact(value) => &host == value,
            HostPattern::Subdomain(suffix) => {
                host.len() > suffix.len()
                    && host.ends_with(suffix)
                    && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
            }
        }
    }
}

/// Parsed allowlist for one plugin. Empty by default: fail closed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostAllowlist {
    patterns: Vec<HostPattern>,
}

impl HostAllowlist {
    pub fn parse(entries: &[String]) -> Result<Self> {
        let mut patterns = Vec::with_capacity(entries.len());
        for entry in entries {
            patterns.push(HostPattern::parse(entry)?);
        }
        Ok(Self { patterns })
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn permits(&self, host: &str) -> bool {
        self.patterns.iter().any(|pattern| pattern.matches(host))
    }

    pub fn patterns(&self) -> &[HostPattern] {
        &self.patterns
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(entries: &[&str]) -> HostAllowlist {
        let owned: Vec<String> = entries.iter().map(|e| (*e).to_string()).collect();
        HostAllowlist::parse(&owned).expect("valid allowlist")
    }

    #[test]
    fn exact_host_matches_case_insensitively() {
        let list = allow(&["API.Example.com"]);
        assert!(list.permits("api.example.com"));
        assert!(list.permits("Api.Example.COM"));
        assert!(!list.permits("api.example.com.evil.test"));
    }

    #[test]
    fn wildcard_covers_subdomains_only() {
        let list = allow(&["*.corp.example"]);
        assert!(list.permits("jira.corp.example"));
        assert!(list.permits("a.b.corp.example"));
        assert!(!list.permits("corp.example"));
        // Suffix confusion: `evilcorp.example` must not match `*.corp.example`.
        assert!(!list.permits("evilcorp.example"));
    }

    #[test]
    fn empty_allowlist_denies_everything() {
        let list = HostAllowlist::default();
        assert!(list.is_empty());
        assert!(!list.permits("api.example.com"));
    }

    #[test]
    fn invalid_entries_are_rejected() {
        for bad in [
            "https://api.example.com",
            "api.example.com/v1",
            "*.",
            "a.*.b",
            "api:8443",
        ] {
            assert!(HostPattern::parse(bad).is_err(), "{bad} should be rejected");
        }
    }
}
