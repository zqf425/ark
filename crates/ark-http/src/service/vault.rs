//! Credential vault: owner-scoped, write-once, no plaintext read API (7.5.5).
//!
//! The plugin references a *name*; only the bus resolves it while building the
//! outbound request. Names are namespaced as `{plugin_id}/{name}`, so a plugin
//! cannot reference another plugin's secret even if it guesses the name.

use crate::domain::fingerprint::fingerprint;
use ark_common::{ArkError, Result};
use ark_ports::CredentialService;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// One stored secret plus the owning plugin.
#[derive(Debug, Clone)]
struct Secret {
    owner: String,
    value: String,
}

/// In-memory vault. `arkd` may swap the storage layer later; the surface must
/// stay handle-only (`store` / `has` / `inject_header`).
#[derive(Debug, Default)]
pub struct CredentialVault {
    secrets: Mutex<BTreeMap<String, Secret>>,
}

impl CredentialVault {
    pub fn new() -> Self {
        Self::default()
    }

    /// Full key for a plugin-local name. Rejects `..` and `/` escapes.
    pub fn key(plugin_id: &str, name: &str) -> Result<String> {
        let name = name.trim();
        if name.is_empty() || name.len() > 128 {
            return Err(ArkError::validation(
                "credential name must be 1..=128 chars",
            ));
        }
        let safe = name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | ':'));
        if !safe || name.contains("..") || name.starts_with('/') || name.ends_with('/') {
            return Err(ArkError::validation(format!(
                "invalid credential name {name:?}"
            )));
        }
        Ok(format!("{plugin_id}/{name}"))
    }

    /// Store/overwrite a secret owned by `plugin_id`.
    pub fn store_for(&self, plugin_id: &str, name: &str, secret: &str) -> Result<()> {
        if secret.is_empty() {
            return Err(ArkError::validation("credential value must not be empty"));
        }
        let key = Self::key(plugin_id, name)?;
        let mut guard = self
            .secrets
            .lock()
            .map_err(|_| ArkError::internal("credential vault poisoned"))?;
        guard.insert(
            key,
            Secret {
                owner: plugin_id.to_string(),
                value: secret.to_string(),
            },
        );
        Ok(())
    }

    /// Does `plugin_id` own `name`?
    pub fn has_for(&self, plugin_id: &str, name: &str) -> bool {
        let Ok(key) = Self::key(plugin_id, name) else {
            return false;
        };
        self.secrets
            .lock()
            .map(|guard| guard.contains_key(&key))
            .unwrap_or(false)
    }

    /// Resolve for injection. Returns `(header_value, log_label)`; the label
    /// carries only `name@hash-prefix`, never the plaintext.
    pub fn resolve(&self, plugin_id: &str, name: &str) -> Result<(String, String)> {
        let key = Self::key(plugin_id, name)?;
        let guard = self
            .secrets
            .lock()
            .map_err(|_| ArkError::internal("credential vault poisoned"))?;
        let secret = guard.get(&key).ok_or_else(|| {
            ArkError::new(
                ark_common::ArkErrorCode::Unauthorized,
                format!("credential {name:?} is not available to {plugin_id}"),
            )
            .with_detail("credential_denied")
        })?;
        if secret.owner != plugin_id {
            return Err(ArkError::new(
                ark_common::ArkErrorCode::Unauthorized,
                format!("credential {name:?} is owned by another plugin"),
            )
            .with_detail("credential_denied"));
        }
        Ok((secret.value.clone(), fingerprint(name, &secret.value)))
    }

    /// Remove every secret owned by a plugin (unload path).
    pub fn purge_owner(&self, plugin_id: &str) -> usize {
        let Ok(mut guard) = self.secrets.lock() else {
            return 0;
        };
        let before = guard.len();
        guard.retain(|_, secret| secret.owner != plugin_id);
        before - guard.len()
    }

    pub fn len(&self) -> usize {
        self.secrets.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl CredentialService for CredentialVault {
    /// `CredentialService` is the host-side trait used by platform services: it
    /// has no plugin argument, so only owner-qualified keys are addressable.
    fn store(&self, name: &str, secret: &str) -> Result<()> {
        match name.split_once('/') {
            Some((owner, rest)) => self.store_for(owner, rest, secret),
            None => Err(ArkError::validation(
                "credential key must be owner-qualified as <plugin_id>/<name>",
            )),
        }
    }

    fn has(&self, name: &str) -> bool {
        match name.split_once('/') {
            Some((owner, rest)) => self.has_for(owner, rest),
            None => false,
        }
    }

    fn inject_header(&self, name: &str) -> Result<Option<String>> {
        let Some((owner, rest)) = name.split_once('/') else {
            return Err(ArkError::validation(
                "credential key must be owner-qualified as <plugin_id>/<name>",
            ));
        };
        self.resolve(owner, rest).map(|(value, _)| Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_plugin_cannot_read_another_plugins_secret() {
        let vault = CredentialVault::new();
        vault
            .store_for("ark.example.jira", "token", "jira-secret")
            .unwrap();
        assert!(vault.resolve("ark.example.jira", "token").is_ok());
        let err = vault.resolve("ark.example.evil", "token").unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("credential_denied"));
    }

    #[test]
    fn log_label_is_name_plus_hash_prefix() {
        let vault = CredentialVault::new();
        vault
            .store_for("ark.example.jira", "token", "plaintext-value")
            .unwrap();
        let (value, label) = vault.resolve("ark.example.jira", "token").unwrap();
        assert_eq!(value, "plaintext-value");
        assert!(label.starts_with("token@"));
        assert!(!label.contains("plaintext-value"));
        assert_eq!(label.len(), "token@".len() + 8);
    }

    #[test]
    fn names_cannot_escape_the_owner_namespace() {
        let vault = CredentialVault::new();
        for bad in ["", "  ", "../other/token", "/abs/token", "a/../b", "tok en"] {
            assert!(
                vault.store_for("ark.example.jira", bad, "v").is_err(),
                "{bad:?} should be rejected"
            );
        }
        // A name that looks qualified stays under the caller's namespace.
        vault
            .store_for("ark.example.jira", "other/token", "v")
            .unwrap();
        assert!(!vault.has_for("ark.example.evil", "other/token"));
        assert!(vault.has_for("ark.example.jira", "other/token"));
    }

    #[test]
    fn platform_trait_requires_owner_qualified_keys() {
        let vault = CredentialVault::new();
        assert!(CredentialService::store(&vault, "token", "v").is_err());
        CredentialService::store(&vault, "ark.example.jira/token", "v").unwrap();
        assert!(CredentialService::has(&vault, "ark.example.jira/token"));
        assert!(!CredentialService::has(&vault, "ark.example.evil/token"));
        let injected = CredentialService::inject_header(&vault, "ark.example.jira/token")
            .unwrap()
            .unwrap();
        assert_eq!(injected, "v");
    }

    #[test]
    fn purge_scopes_to_one_plugin() {
        let vault = CredentialVault::new();
        vault.store_for("a", "k", "1").unwrap();
        vault.store_for("b", "k", "2").unwrap();
        assert_eq!(vault.purge_owner("a"), 1);
        assert_eq!(vault.len(), 1);
        assert!(vault.has_for("b", "k"));
    }
}
