//! The persisted `providers.json` document.
//!
//! One file, one shape: the provider table plus which id is the default. Keeping
//! the default inside the same document means a crash between two writes cannot
//! leave a default pointing at a provider that was never stored.

use ark_contract::{ProviderEntry, ProviderView};
use serde::{Deserialize, Serialize};

use crate::domain::{api_key_hint, validate_endpoint_format};

/// Wire/disk shape of `providers.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProvidersDocument {
    #[serde(default)]
    pub providers: Vec<ProviderEntry>,
    #[serde(default)]
    pub default_provider_id: Option<String>,
}

impl ProvidersDocument {
    pub fn get(&self, id: &str) -> Option<&ProviderEntry> {
        self.providers.iter().find(|entry| entry.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ProviderEntry> {
        self.providers.iter_mut().find(|entry| entry.id == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    /// Insert or replace, preserving position on update so the file does not
    /// reorder on every edit.
    pub fn put(&mut self, entry: ProviderEntry) {
        match self.get_mut(&entry.id) {
            Some(slot) => *slot = entry,
            None => self.providers.push(entry),
        }
    }

    /// Remove an entry. Returns the removed entry.
    pub fn remove(&mut self, id: &str) -> Option<ProviderEntry> {
        let index = self.providers.iter().position(|entry| entry.id == id)?;
        Some(self.providers.remove(index))
    }

    /// The default id, if it still names a stored provider.
    pub fn effective_default(&self) -> Option<&str> {
        let id = self.default_provider_id.as_deref()?;
        self.contains(id).then_some(id)
    }

    /// Project one stored entry into its redacted view.
    ///
    /// This is the only place a `ProviderView` is built, so there is exactly one
    /// path from the secret to the wire and it always masks.
    pub fn view_of(&self, entry: &ProviderEntry) -> ProviderView {
        let format = validate_endpoint_format(&entry.endpoint_format)
            .unwrap_or_default()
            .as_str()
            .to_string();
        ProviderView {
            id: entry.id.clone(),
            label: entry.label.clone(),
            base_url: entry.base_url.clone(),
            endpoint_format: format,
            model: entry.model.clone(),
            enabled: entry.enabled,
            is_default: self.effective_default() == Some(entry.id.as_str()),
            has_api_key: api_key_hint(entry.api_key.as_deref()).is_some(),
            api_key_hint: api_key_hint(entry.api_key.as_deref()),
        }
    }

    /// Project every entry, sorted by id so the GUI list is stable.
    pub fn views(&self) -> Vec<ProviderView> {
        let mut entries: Vec<&ProviderEntry> = self.providers.iter().collect();
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        entries
            .into_iter()
            .map(|entry| self.view_of(entry))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> ProviderEntry {
        ProviderEntry::new(id, "model-x")
    }

    #[test]
    fn put_replaces_in_place_and_keeps_order() {
        let mut doc = ProvidersDocument::default();
        doc.put(entry("a"));
        doc.put(entry("b"));
        let mut updated = entry("a");
        updated.model = "model-y".into();
        doc.put(updated);
        assert_eq!(doc.providers.len(), 2);
        assert_eq!(doc.providers[0].id, "a");
        assert_eq!(doc.providers[0].model, "model-y");
    }

    #[test]
    fn remove_reports_what_it_removed() {
        let mut doc = ProvidersDocument::default();
        doc.put(entry("a"));
        assert_eq!(doc.remove("a").map(|removed| removed.id), Some("a".into()));
        assert!(doc.remove("a").is_none());
    }

    #[test]
    fn the_effective_default_must_exist() {
        let mut doc = ProvidersDocument::default();
        doc.put(entry("a"));
        doc.default_provider_id = Some("missing".into());
        assert_eq!(doc.effective_default(), None);
        doc.default_provider_id = Some("a".into());
        assert_eq!(doc.effective_default(), Some("a"));
    }

    #[test]
    fn views_never_carry_the_secret() {
        let mut doc = ProvidersDocument::default();
        let mut with_key = entry("a");
        with_key.api_key = Some("sk-1234567890abcdef".into());
        doc.put(with_key);
        doc.default_provider_id = Some("a".into());
        let view = &doc.views()[0];
        assert!(view.has_api_key);
        assert_eq!(view.api_key_hint.as_deref(), Some("sk-1***ef"));
        assert!(view.is_default);
        let json = serde_json::to_string(&doc.views()).unwrap();
        assert!(!json.contains("567890"), "{json}");
    }

    #[test]
    fn views_are_sorted_by_id() {
        let mut doc = ProvidersDocument::default();
        doc.put(entry("z"));
        doc.put(entry("a"));
        let ids: Vec<String> = doc.views().into_iter().map(|view| view.id).collect();
        assert_eq!(ids, vec!["a".to_string(), "z".to_string()]);
    }
}
