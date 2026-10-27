//! Plugin egress registration: version + allowlist per plugin id (7.5.1).
//!
//! A plugin must be registered before it may send anything. Unknown plugin ids
//! fail closed instead of falling back to a global allowlist.

use crate::domain::allowlist::HostAllowlist;
use crate::domain::request_guard::validate_plugin_id;
use ark_common::{ArkError, Result};
use std::collections::BTreeMap;

/// What the bus knows about one plugin's outbound rights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressGrant {
    pub plugin_id: String,
    pub version: String,
    pub allowlist: HostAllowlist,
}

/// Registration table, owned by the bus.
#[derive(Debug, Default)]
pub struct EgressRegistry {
    grants: BTreeMap<String, EgressGrant>,
}

impl EgressRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a plugin's egress. Re-registering the same id replaces the
    /// grant, which is what an unload/reload cycle needs.
    pub fn register(
        &mut self,
        plugin_id: &str,
        version: &str,
        allow: &[String],
    ) -> Result<EgressGrant> {
        validate_plugin_id(plugin_id)?;
        if version.trim().is_empty() {
            return Err(ArkError::validation("plugin version is empty"));
        }
        let allowlist = HostAllowlist::parse(allow).map_err(|err| {
            ArkError::validation(format!(
                "invalid http allowlist for {plugin_id}: {}",
                err.message
            ))
        })?;
        let grant = EgressGrant {
            plugin_id: plugin_id.to_string(),
            version: version.trim().to_string(),
            allowlist,
        };
        self.grants.insert(plugin_id.to_string(), grant.clone());
        Ok(grant)
    }

    pub fn unregister(&mut self, plugin_id: &str) -> bool {
        self.grants.remove(plugin_id).is_some()
    }

    pub fn get(&self, plugin_id: &str) -> Option<&EgressGrant> {
        self.grants.get(plugin_id)
    }

    /// Fail-closed lookup used on the request path.
    pub fn require(&self, plugin_id: &str) -> Result<&EgressGrant> {
        self.grants.get(plugin_id).ok_or_else(|| {
            ArkError::new(
                ark_common::ArkErrorCode::Unauthorized,
                format!("plugin {plugin_id} is not registered for http egress"),
            )
            .with_detail("empty_allowlist")
        })
    }

    pub fn len(&self) -> usize {
        self.grants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_plugin_is_denied() {
        let registry = EgressRegistry::new();
        let err = registry.require("ark.example.jira").unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("empty_allowlist"));
    }

    #[test]
    fn invalid_allowlist_entry_blocks_registration() {
        let mut registry = EgressRegistry::new();
        assert!(registry
            .register("ark.example.jira", "1.0.0", &["https://x.test".to_string()])
            .is_err());
        assert!(registry.is_empty());
    }

    #[test]
    fn invalid_plugin_id_blocks_registration() {
        let mut registry = EgressRegistry::new();
        assert!(registry.register("Ark.Builtin", "1.0.0", &[]).is_err());
        assert!(registry.register("ark.builtin", "", &[]).is_err());
    }

    #[test]
    fn reregister_replaces_and_unregister_removes() {
        let mut registry = EgressRegistry::new();
        registry
            .register(
                "ark.example.jira",
                "1.0.0",
                &["api.example.com".to_string()],
            )
            .unwrap();
        registry
            .register(
                "ark.example.jira",
                "1.1.0",
                &["other.example.com".to_string()],
            )
            .unwrap();
        let grant = registry.require("ark.example.jira").unwrap();
        assert_eq!(grant.version, "1.1.0");
        assert!(grant.allowlist.permits("other.example.com"));
        assert!(registry.unregister("ark.example.jira"));
        assert!(!registry.unregister("ark.example.jira"));
        assert_eq!(registry.len(), 0);
    }
}
