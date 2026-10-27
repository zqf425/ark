//! Plugin identity: host-injected, not self-reported (design 7.5.5).
//!
//! A plugin cannot forge its identity for two independent reasons:
//!
//! 1. [`PluginIdentity`] has **private fields and no public constructor**; the
//!    only way to obtain one is [`PluginIdentity::host_issue`], which is called
//!    by the host's loader on the manifest that was just read.
//! 2. Every capability handle is built from an identity the host owns, so even
//!    a hand-rolled identity would not address another plugin's namespace:
//!    config keys, KV keys, file roots and credential names all carry the
//!    *handle's* owner, not a caller-supplied string.

use ark_common::{ArkError, Result};
use serde::{Deserialize, Serialize};

/// Validate a plugin id: lowercase machine identity only.
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

/// What the host granted a plugin. Kept with the identity so a capability can
/// check a permission without a second lookup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Manifest capabilities that were actually granted.
    #[serde(default)]
    pub capabilities: Vec<ark_contract::Capability>,
    /// May this plugin subscribe to events outside its own namespace?
    #[serde(default)]
    pub global_events: bool,
}

impl Grant {
    pub fn allows(&self, capability: ark_contract::Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// Host-issued identity. Fields are private on purpose: see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginIdentity {
    plugin_id: String,
    version: String,
    grant: Grant,
}

impl PluginIdentity {
    /// The single construction point. `pub(crate)` is deliberate: only the
    /// loader inside this crate can mint an identity, so a plugin crate cannot
    /// call it even though it can name the type.
    pub(crate) fn host_issue(
        plugin_id: impl Into<String>,
        version: impl Into<String>,
        grant: Grant,
    ) -> Result<Self> {
        let plugin_id = plugin_id.into();
        validate_plugin_id(&plugin_id)?;
        let version = version.into();
        if version.trim().is_empty() {
            return Err(ArkError::validation("plugin version is empty"));
        }
        Ok(Self {
            plugin_id,
            version: version.trim().to_string(),
            grant,
        })
    }

    /// Stable machine identity, e.g. `ark.builtin.code-ui`.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Manifest version, used in `User-Agent` and log lines.
    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn grant(&self) -> &Grant {
        &self.grant
    }

    /// Composite label for diagnostics: `<plugin_id>@<version>`.
    pub fn label(&self) -> String {
        format!("{}@{}", self.plugin_id, self.version)
    }
}

/// Build a grant from a manifest (host-side helper).
pub(crate) fn grant_for(manifest: &crate::domain::PluginManifest) -> Grant {
    Grant {
        capabilities: manifest.capabilities.clone(),
        global_events: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_id_shape_is_enforced() {
        for good in ["ark.builtin.code-ui", "ark.example.jira", "p1"] {
            assert!(validate_plugin_id(good).is_ok(), "{good} should be valid");
        }
        for bad in [
            "",
            "Ark.Builtin",
            "ark..builtin",
            ".ark",
            "ark/evil",
            "ark builtin",
        ] {
            assert!(validate_plugin_id(bad).is_err(), "{bad} should be invalid");
        }
    }

    #[test]
    fn host_issued_identity_carries_version_and_grant() {
        let grant = Grant {
            capabilities: vec![ark_contract::Capability::Tool],
            global_events: false,
        };
        let identity = PluginIdentity::host_issue("ark.example.jira", "1.2.3", grant).unwrap();
        assert_eq!(identity.plugin_id(), "ark.example.jira");
        assert_eq!(identity.version(), "1.2.3");
        assert_eq!(identity.label(), "ark.example.jira@1.2.3");
        assert!(identity.grant().allows(ark_contract::Capability::Tool));
        assert!(!identity.grant().allows(ark_contract::Capability::Backend));
    }

    #[test]
    fn identity_rejects_invalid_ids_and_empty_versions() {
        assert!(PluginIdentity::host_issue("Ark.Bad", "1.0.0", Grant::default()).is_err());
        assert!(PluginIdentity::host_issue("ark.a", "  ", Grant::default()).is_err());
    }

    #[test]
    fn grant_comes_from_the_manifest_not_the_plugin() {
        let mut manifest = crate::domain::PluginManifest::new(
            "ark.example.jira",
            "1.0.0",
            crate::PluginKind::Tool,
        )
        .unwrap();
        manifest.capabilities = vec![ark_contract::Capability::Tool];
        assert_eq!(
            grant_for(&manifest),
            Grant {
                capabilities: vec![ark_contract::Capability::Tool],
                global_events: false,
            }
        );
    }
}
