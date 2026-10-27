//! `ProviderStore`: the single mutation point for the provider table (§7.2).
//!
//! Reads clone a small immutable snapshot out of the mutex, so a caller that
//! mutates and then reads in the same breath always sees the new state: there is
//! no cached client, no versioned copy, and no deferred flush to go stale.
//!
//! Writes follow one order — validate, persist, then swap the in-memory snapshot.
//! A failed write therefore leaves memory and disk both untouched, and a
//! successful one leaves them identical.
//!
//! Secrets never leave this module unmasked: [`ProviderStore::list`] projects
//! through [`ProvidersDocument::view_of`], and the only other exit
//! ([`ProviderStore::resolve`]) hands the key to `ark-llm` in-process.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use ark_common::{ArkError, Result};
use ark_contract::{
    ProviderEntry, ProviderListResponse, ProviderTestRequest, ProviderTestResponse,
};
use ark_llm::ProviderConfig;
use tracing::warn;

use crate::config::providers_path;
use crate::domain::{normalize_entry, validate_endpoint_format, ProvidersDocument as Document};
use crate::service::entry_config::config_from_entry;
use crate::service::persist::{load_document, write_document, DocumentLoad};
use crate::service::probe;

/// The provider routing table: CRUD, validation, masking and connectivity probe.
///
/// # Secret storage
///
/// `api_key` is persisted in `providers.json` next to the rest of the entry, with
/// the same file permissions as ordinary configuration. This is a deliberate v1
/// simplification; the follow-up is to move the secret into the OS keyring
/// (Windows Credential Manager / macOS Keychain) and keep only a reference in the
/// table. Until then, the store must not be handed a path inside a shared or
/// world-readable directory.
pub struct ProviderStore {
    path: PathBuf,
    /// Snapshot of the persisted document. Swapped wholesale after a write.
    inner: Mutex<Document>,
}

impl std::fmt::Debug for ProviderStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderStore")
            .field("path", &self.path)
            .field("providers", &self.lock().providers.len())
            .finish_non_exhaustive()
    }
}

impl ProviderStore {
    /// Open the table at `<data_dir>/providers.json`.
    ///
    /// A missing file is an empty table. A corrupt file is also treated as empty,
    /// but warns loudly: refusing to start would lock the user out of the very
    /// dialog needed to fix it.
    pub fn new(data_dir: PathBuf) -> Self {
        let path = providers_path(&data_dir);
        let document = match load_document(&path) {
            DocumentLoad::Loaded(document) => document,
            DocumentLoad::Missing => Document::default(),
            DocumentLoad::Unusable(reason) => {
                warn!(path = %path.display(), %reason, "provider table is unreadable");
                Document::default()
            }
        };
        Self {
            path,
            inner: Mutex::new(document),
        }
    }

    /// Path of the backing file.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn lock(&self) -> MutexGuard<'_, Document> {
        // A poisoned lock only means another thread panicked mid-mutation; the
        // snapshot is still a consistent value, so recover it.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The redacted list view. Never contains a plaintext key.
    pub fn list(&self) -> ProviderListResponse {
        let document = self.lock();
        let providers = document.views();
        let default_provider_id = document.effective_default().map(str::to_string);
        // The process binds to the default entry through `resolve(None)`, so the
        // active id is exactly the default that can actually serve a request.
        let active_provider_id = default_provider_id
            .as_deref()
            .and_then(|id| document.get(id))
            .and_then(config_from_entry)
            .map(|config| config.provider_id);
        drop(document);
        ProviderListResponse {
            providers,
            default_provider_id,
            active_provider_id,
        }
    }

    /// Add or replace a provider. Returns the updated list.
    ///
    /// `make_default` promotes the entry to the default in the same write, so a
    /// first-time setup is one round trip instead of two.
    pub fn upsert(&self, entry: ProviderEntry, make_default: bool) -> Result<ProviderListResponse> {
        let candidate = normalize_entry(entry)?;
        let document = self.lock();
        let mut next = document.clone();
        let previous_key = next.get(&candidate.id).and_then(|old| old.api_key.clone());
        let entry = keep_existing_key(candidate, previous_key);
        next.put(entry.clone());
        if make_default {
            // Enabling and promoting in one call is the settings dialog's
            // "add provider" path; a disabled entry is never made default.
            if entry.enabled {
                next.default_provider_id = Some(entry.id);
            }
        }
        self.commit(document, next)
    }

    /// Remove a provider. Deleting the default clears the default.
    pub fn delete(&self, id: &str) -> Result<ProviderListResponse> {
        let id = id.trim();
        if id.is_empty() {
            return Err(ArkError::validation("provider id is required"));
        }
        let document = self.lock();
        if !document.contains(id) {
            return Err(ArkError::not_found(format!(
                "provider `{id}` does not exist"
            )));
        }
        let mut next = document.clone();
        next.remove(id);
        if next.default_provider_id.as_deref() == Some(id) {
            next.default_provider_id = None;
        }
        self.commit(document, next)
    }

    /// Name the default provider. It must exist and be enabled.
    pub fn set_default(&self, id: &str) -> Result<ProviderListResponse> {
        let id = id.trim();
        if id.is_empty() {
            return Err(ArkError::validation("provider id is required"));
        }
        let document = self.lock();
        let Some(entry) = document.get(id) else {
            return Err(ArkError::not_found(format!(
                "provider `{id}` does not exist"
            )));
        };
        if !entry.enabled {
            return Err(ArkError::validation(format!(
                "provider `{id}` is disabled; enable it before making it the default"
            )));
        }
        let mut next = document.clone();
        next.default_provider_id = Some(id.to_string());
        self.commit(document, next)
    }

    /// Resolve a provider into client configuration.
    ///
    /// `None` selects the default. The result is `None` when nothing is
    /// configured, when the named provider does not exist, or when the entry is
    /// disabled / has no key: an unusable provider must not become a client with
    /// an empty credential.
    pub fn resolve(&self, id: Option<&str>) -> Option<ProviderConfig> {
        let document = self.lock();
        let target = match id.map(str::trim).filter(|value| !value.is_empty()) {
            Some(explicit) => document.get(explicit),
            None => document
                .effective_default()
                .and_then(|default| document.get(default)),
        }?;
        config_from_entry(target)
    }

    /// Probe connectivity with one minimal request.
    ///
    /// `request.entry` wins so the settings dialog can test a draft before it is
    /// saved; `request.provider_id` then falls back to the stored table.
    pub async fn test(&self, request: ProviderTestRequest) -> ProviderTestResponse {
        let entry = match self.entry_for_probe(request) {
            Ok(entry) => entry,
            Err(response) => return response,
        };
        match validate_for_probe(&entry) {
            Ok(()) => {}
            Err(message) => return probe::unusable(message, None),
        }
        match config_from_entry(&entry) {
            Some(config) => probe::probe(config).await,
            None => probe::unusable(
                format!(
                    "provider `{}` cannot be used: it is disabled or has no api key",
                    entry.id
                ),
                None,
            ),
        }
    }

    /// Pick the entry to probe, reusing a stored key for a draft that omits it.
    fn entry_for_probe(
        &self,
        request: ProviderTestRequest,
    ) -> std::result::Result<ProviderEntry, ProviderTestResponse> {
        if let Some(draft) = request.entry {
            let normalized = normalize_entry(draft).map_err(|error| {
                probe::unusable(
                    format!("cannot test this configuration: {}", error.message),
                    None,
                )
            })?;
            if normalized.api_key.is_some() {
                return Ok(normalized);
            }
            let stored_key = self
                .lock()
                .get(&normalized.id)
                .and_then(|stored| stored.api_key.clone());
            let mut with_key = normalized;
            with_key.api_key = stored_key;
            return Ok(with_key);
        }
        let Some(id) = request
            .provider_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            return Err(probe::unusable(
                "nothing to test: send a provider entry or a provider id",
                None,
            ));
        };
        let document = self.lock();
        match document.get(&id) {
            Some(entry) => Ok(entry.clone()),
            None => Err(probe::unusable(
                format!("provider `{id}` does not exist"),
                None,
            )),
        }
    }

    /// Persist `next`, then publish it as the current snapshot.
    fn commit(
        &self,
        mut guard: MutexGuard<'_, Document>,
        next: Document,
    ) -> Result<ProviderListResponse> {
        write_document(&self.path, &next)?;
        *guard = next;
        let list = {
            let providers = guard.views();
            let default_provider_id = guard.effective_default().map(str::to_string);
            let active_provider_id = default_provider_id
                .as_deref()
                .and_then(|id| guard.get(id))
                .and_then(config_from_entry)
                .map(|config| config.provider_id);
            ProviderListResponse {
                providers,
                default_provider_id,
                active_provider_id,
            }
        };
        Ok(list)
    }
}

/// A GUI edit sends no key, which means "keep the stored one".
fn keep_existing_key(mut candidate: ProviderEntry, previous: Option<String>) -> ProviderEntry {
    if candidate.api_key.is_none() {
        candidate.api_key = previous;
    }
    candidate
}

/// Reject a probe target early, with a message an operator can act on.
fn validate_for_probe(entry: &ProviderEntry) -> std::result::Result<(), String> {
    validate_endpoint_format(&entry.endpoint_format).map_err(|error| error.message)?;
    if entry.api_key.as_deref().unwrap_or("").trim().is_empty() {
        return Err(format!(
            "provider `{}` has no api key; save or enter one before testing",
            entry.id
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::ProviderEntry;
    use std::path::Path;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ark-provider-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(id: &str, key: Option<&str>) -> ProviderEntry {
        let mut entry = ProviderEntry::new(id, "deepseek-chat");
        entry.base_url = "https://api.deepseek.com".into();
        entry.api_key = key.map(str::to_string);
        entry.endpoint_format = "openai_chat".into();
        entry
    }

    fn store(dir: &Path) -> ProviderStore {
        ProviderStore::new(dir.to_path_buf())
    }

    #[test]
    fn a_blank_id_is_a_validation_error() {
        let dir = temp_dir("blank-id");
        let err = store(&dir)
            .upsert(entry("  ", Some("sk-1234567890")), false)
            .unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::ValidationError);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_invalid_endpoint_format_is_a_validation_error() {
        let dir = temp_dir("bad-format");
        let mut bad = entry("p", Some("sk-1234567890"));
        bad.endpoint_format = "grpc".into();
        let err = store(&dir).upsert(bad, false).unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::ValidationError);
        assert!(err.message.contains("openai_chat"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_out_of_range_temperature_is_rejected() {
        let dir = temp_dir("temp");
        let mut hot = entry("p", Some("sk-1234567890"));
        hot.temperature = Some(2.5);
        let err = store(&dir).upsert(hot, false).unwrap_err();
        assert!(err.message.contains("temperature must be between 0 and 2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_never_returns_the_plaintext_key() {
        let dir = temp_dir("mask");
        let store = store(&dir);
        store
            .upsert(entry("p", Some("sk-1234567890abcdef")), false)
            .unwrap();
        let list = store.list();
        assert_eq!(list.providers.len(), 1);
        assert!(list.providers[0].has_api_key);
        assert_eq!(list.providers[0].api_key_hint.as_deref(), Some("sk-1***ef"));
        let json = serde_json::to_string(&list).unwrap();
        assert!(!json.contains("567890"), "{json}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_short_key_is_masked_entirely() {
        let dir = temp_dir("short-key");
        let store = store(&dir);
        store.upsert(entry("p", Some("sk-123")), false).unwrap();
        let list = store.list();
        assert_eq!(list.providers[0].api_key_hint.as_deref(), Some("***"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_upsert_without_a_key_keeps_the_stored_one() {
        let dir = temp_dir("keep-key");
        let store = store(&dir);
        store
            .upsert(entry("p", Some("sk-1234567890abcdef")), false)
            .unwrap();
        let mut edit = entry("p", None);
        edit.model = "deepseek-reasoner".into();
        let list = store.upsert(edit, false).unwrap();
        assert!(list.providers[0].has_api_key);
        assert_eq!(
            store.resolve(Some("p")).unwrap().api_key,
            "sk-1234567890abcdef"
        );
        assert_eq!(store.resolve(Some("p")).unwrap().model, "deepseek-reasoner");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_the_default_clears_it_without_panicking() {
        let dir = temp_dir("delete-default");
        let store = store(&dir);
        store
            .upsert(entry("p", Some("sk-1234567890")), true)
            .unwrap();
        assert_eq!(store.list().default_provider_id.as_deref(), Some("p"));
        let list = store.delete("p").unwrap();
        assert_eq!(list.default_provider_id, None);
        assert!(list.providers.is_empty());
        assert!(store.resolve(None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_an_unknown_provider_is_not_found() {
        let dir = temp_dir("delete-missing");
        let err = store(&dir).delete("ghost").unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_default_must_exist_and_be_enabled() {
        let dir = temp_dir("set-default");
        let store = store(&dir);
        assert!(store.set_default("ghost").is_err());
        let mut off = entry("off", Some("sk-1234567890"));
        off.enabled = false;
        store.upsert(off, false).unwrap();
        let err = store.set_default("off").unwrap_err();
        assert!(err.message.contains("disabled"), "{}", err.message);
        store
            .upsert(entry("on", Some("sk-1234567890")), false)
            .unwrap();
        assert_eq!(
            store
                .set_default("on")
                .unwrap()
                .default_provider_id
                .as_deref(),
            Some("on")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_is_idempotent_and_the_document_round_trips() {
        let dir = temp_dir("roundtrip");
        {
            let store = store(&dir);
            store
                .upsert(entry("p", Some("sk-1234567890abcdef")), true)
                .unwrap();
            store
                .upsert(entry("p", Some("sk-1234567890abcdef")), true)
                .unwrap();
            assert_eq!(store.list().providers.len(), 1);
        }
        // A fresh store reads the same table back: memory and disk agree.
        let reopened = store(&dir);
        let list = reopened.list();
        assert_eq!(list.providers.len(), 1);
        assert_eq!(list.default_provider_id.as_deref(), Some("p"));
        assert_eq!(list.active_provider_id.as_deref(), Some("p"));
        let bytes = std::fs::read(dir.join("providers.json")).unwrap();
        assert!(
            !bytes.starts_with(&ark_common::UTF8_BOM_BYTES),
            "file must not carry a BOM"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_follows_the_default_and_the_explicit_id() {
        let dir = temp_dir("resolve");
        let store = store(&dir);
        store
            .upsert(entry("a", Some("sk-1234567890a")), true)
            .unwrap();
        store
            .upsert(entry("b", Some("sk-1234567890b")), false)
            .unwrap();
        assert_eq!(store.resolve(None).unwrap().provider_id, "a");
        assert_eq!(store.resolve(Some("b")).unwrap().provider_id, "b");
        assert!(store.resolve(Some("ghost")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resolve_is_none_without_a_key() {
        let dir = temp_dir("no-key");
        let store = store(&dir);
        store.upsert(entry("p", None), true).unwrap();
        assert!(store.resolve(None).is_none());
        let list = store.list();
        assert!(!list.providers[0].has_api_key);
        assert_eq!(list.providers[0].api_key_hint, None);
        // The default is still recorded, but nothing is bound to it.
        assert_eq!(list.default_provider_id.as_deref(), Some("p"));
        assert_eq!(list.active_provider_id, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn testing_an_unknown_provider_does_not_hit_the_network() {
        let dir = temp_dir("probe-missing");
        let store = store(&dir);
        let response = store
            .test(ProviderTestRequest {
                provider_id: Some("ghost".into()),
                entry: None,
            })
            .await;
        assert!(!response.ok);
        assert_eq!(response.status, None);
        assert!(
            response.message.contains("does not exist"),
            "{}",
            response.message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn testing_a_keyless_draft_is_rejected_before_any_request() {
        let dir = temp_dir("probe-no-key");
        let store = store(&dir);
        let response = store
            .test(ProviderTestRequest {
                provider_id: None,
                entry: Some(entry("draft", None)),
            })
            .await;
        assert!(!response.ok);
        assert!(
            response.message.contains("no api key"),
            "{}",
            response.message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_draft_probe_reuses_the_stored_key() {
        let dir = temp_dir("probe-draft");
        let store = store(&dir);
        store
            .upsert(entry("p", Some("sk-1234567890")), false)
            .unwrap();
        // No key in the draft: the entry passed validation for the probe instead
        // of failing on a missing credential.
        let mut draft = entry("p", None);
        draft.base_url = "http://127.0.0.1:1/v1".into();
        let response = store
            .test(ProviderTestRequest {
                provider_id: None,
                entry: Some(draft),
            })
            .await;
        assert!(!response.ok);
        assert!(
            !response.message.contains("no api key"),
            "{}",
            response.message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_does_not_panic_on_open() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join("providers.json"), b"{ not json").unwrap();
        let store = store(&dir);
        assert!(store.list().providers.is_empty());
        // A write repairs the file.
        store
            .upsert(entry("p", Some("sk-1234567890")), true)
            .unwrap();
        assert_eq!(store.list().providers.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_store_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProviderStore>();
    }
}
