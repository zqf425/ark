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
//! `list()` projects through [`ProvidersDocument::view_of`] and is always
//! redacted. The explicit [`ProviderStore::get`] returns the full stored entry,
//! including its api key, only for the settings edit form; `resolve()` hands the
//! key to `ark-llm` in-process. Neither exit is logged.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use ark_common::{ArkError, Result};
use ark_contract::{
    ProviderEntry, ProviderListResponse, ProviderModelCatalogConnection,
    ProviderModelCatalogRequest, ProviderModelCatalogResponse, ProviderTestRequest,
    ProviderTestResponse,
};
use ark_llm::{ModelCatalogClient, ProviderConfig};
use tracing::warn;

use crate::config::providers_path;
use crate::domain::{
    normalize_entry, validate_base_url, validate_endpoint_format, ProvidersDocument as Document,
};
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
    /// One reusable client for explicit model-discovery actions.
    catalog: ModelCatalogClient,
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
        Self::with_catalog_client(data_dir, ModelCatalogClient::default())
    }

    /// Open the table with an injected model-catalog client.
    ///
    /// Parameter: `data_dir` owns the provider document; `catalog` carries the
    /// process HTTP pool and transport policy. Returns the initialized store.
    pub fn with_catalog_client(data_dir: PathBuf, catalog: ModelCatalogClient) -> Self {
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
            catalog,
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
    ///
    /// `compaction_threshold_tokens` reports the threshold the runtime derives
    /// for the effective provider: 80% of that provider's context window minus
    /// its reserved output. `context_window_tokens` reports the window the
    /// threshold divides — the declared window or the 256K default
    /// (`ark_llm::DEFAULT_CONTEXT_WINDOW_TOKENS`), the same fallback the
    /// settings inventory applies. `None` only when no provider resolves at
    /// all — the renderer then hides the context meter instead of guessing a
    /// denominator.
    pub fn list(&self) -> ProviderListResponse {
        let document = self.lock();
        let providers = document.views();
        let default_provider_id = document.effective_default().map(str::to_string);
        let default_model = document.effective_default_model();
        // The process binds to the default entry through `resolve(None)`, so the
        // active id is exactly the default that can actually serve a request.
        let resolved = document
            .effective_default()
            .and_then(|id| document.get(id))
            .and_then(config_from_entry);
        let active_provider_id = resolved.as_ref().map(|config| config.provider_id.clone());
        let threshold = resolved
            .as_ref()
            .and_then(ProviderConfig::compaction_threshold);
        let window = resolved
            .as_ref()
            .map(|config| config.effective_context_window());
        drop(document);
        ProviderListResponse {
            providers,
            default_provider_id,
            default_model,
            active_provider_id,
            compaction_threshold_tokens: threshold,
            context_window_tokens: window,
        }
    }

    /// Full entry for the settings edit form, including the stored api key.
    ///
    /// This is intentionally separate from `list()`: the table, capabilities
    /// and diagnostics must stay redacted, while editing a saved provider must
    /// not silently present an empty key.
    pub fn get(&self, id: &str) -> Result<ProviderEntry> {
        let id = id.trim();
        if id.is_empty() {
            return Err(ArkError::validation("provider id is required"));
        }
        self.lock()
            .get(id)
            .cloned()
            .ok_or_else(|| ArkError::not_found(format!("provider `{id}` does not exist")))
    }

    /// Add or replace a provider. Returns the updated list.
    ///
    /// `make_default` promotes the entry to the default in the same write, so a
    /// first-time setup is one round trip instead of two.
    pub fn upsert(&self, entry: ProviderEntry, make_default: bool) -> Result<ProviderListResponse> {
        let candidate = normalize_entry(entry)?;
        let document = self.lock();
        let mut next = document.clone();
        let previous = next.get(&candidate.id).cloned();
        let entry = keep_omitted_edit_fields(candidate, previous.as_ref());
        next.put(entry.clone());
        if make_default {
            // Enabling and promoting in one call is the settings dialog's
            // "add provider" path; a disabled entry is never made default.
            // A pinned default model belongs to the previous provider, so a
            // real switch clears it; re-saving the same default keeps it.
            if entry.enabled {
                let switches = next.default_provider_id.as_deref() != Some(entry.id.as_str());
                next.default_provider_id = Some(entry.id);
                if switches {
                    next.default_model = None;
                }
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
            next.default_model = None;
        }
        self.commit(document, next)
    }

    /// Name the default provider, optionally pinning one of its models.
    ///
    /// `model` must be the entry's stored model or an inventory row id: the
    /// pinned default is only ever an override of what that provider serves,
    /// never a free-form id. `None` (or blank) pins the provider alone and
    /// clears any previous pin so the entry's stored `model` applies again.
    pub fn set_default(&self, id: &str, model: Option<&str>) -> Result<ProviderListResponse> {
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
        let model = model.map(str::trim).filter(|value| !value.is_empty());
        if let Some(model) = model {
            let known = model == entry.model.trim()
                || entry.models.iter().flatten().any(|item| item.id == model);
            if !known {
                return Err(ArkError::validation(format!(
                    "provider `{id}` does not serve model `{model}`; import it into the inventory first"
                )));
            }
        }
        let mut next = document.clone();
        next.default_provider_id = Some(id.to_string());
        next.default_model = model.map(str::to_string);
        self.commit(document, next)
    }

    /// Resolve a provider into client configuration.
    ///
    /// `None` selects the default. The result is `None` when nothing is
    /// configured, when the named provider does not exist, or when the entry is
    /// disabled / has no key: an unusable provider must not become a client with
    /// an empty credential.
    ///
    /// When the default resolves, a pinned default model (see `set_default`)
    /// replaces the entry's stored `model`, and the context window follows the
    /// inventory row recorded for that model.
    pub fn resolve(&self, id: Option<&str>) -> Option<ProviderConfig> {
        let document = self.lock();
        let is_default = id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none();
        let target = match id.map(str::trim).filter(|value| !value.is_empty()) {
            Some(explicit) => document.get(explicit),
            None => document
                .effective_default()
                .and_then(|default| document.get(default)),
        }?;
        let mut config = config_from_entry(target)?;
        if is_default {
            if let Some(model) = document.effective_default_model() {
                if model != config.model {
                    config.model = model.clone();
                    config.context_window_tokens = target
                        .models
                        .iter()
                        .flatten()
                        .find(|item| item.id == model)
                        .and_then(|item| item.context_window_tokens);
                }
            }
        }
        Some(config)
    }

    /// Whether the resolved provider's model explicitly declared vision support.
    ///
    /// Same selection rules as [`Self::resolve`], but reads the catalog-reported
    /// `supports_vision` flag. Metadata belongs to the model id, never to the
    /// provider entry: the effective model — a thread override, or the pinned
    /// default model when the default provider resolves — reads the entry-level
    /// flag when it matches the stored model, an inventory row's flag when one
    /// exists, and is unknown otherwise. `None` (unknown) must be treated as
    /// "not declared": image parts are only sent on an explicit positive, never
    /// inherited from another model or guessed from the model name.
    pub fn resolve_vision(&self, id: Option<&str>, model: Option<&str>) -> Option<bool> {
        let document = self.lock();
        let explicit = id.map(str::trim).filter(|value| !value.is_empty());
        let target = match explicit {
            Some(name) => document.get(name),
            None => document
                .effective_default()
                .and_then(|default| document.get(default)),
        }?;
        let model = model
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                explicit
                    .is_none()
                    .then(|| document.effective_default_model())
                    .flatten()
            });
        if let Some(model) = model.as_deref() {
            if model == target.model {
                return target.supports_vision;
            }
            if let Some(item) = target.models.iter().flatten().find(|item| item.id == model) {
                return item.supports_vision;
            }
            // An override without inventory metadata is unknown. Inheriting the
            // entry-level flag here would apply the default model's vision
            // capability to a different model id that never declared it.
            return None;
        }
        target.supports_vision
    }

    /// Resolve a provider for a turn that may override the model.
    ///
    /// Selection rules match [`Self::resolve`]; the override only replaces the
    /// model, and the context window follows the inventory entry recorded for
    /// that model. An override without an inventory entry clears the window —
    /// the stored limit belongs to the stored model id, and compacting against
    /// another model's limit would corrupt the threshold.
    pub fn resolve_for_model(
        &self,
        id: Option<&str>,
        model: Option<String>,
    ) -> Option<ProviderConfig> {
        let mut config = self.resolve(id)?;
        let Some(model) = model
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            return Some(config);
        };
        if model == config.model {
            return Some(config);
        }
        let document = self.lock();
        let target = match id.map(str::trim).filter(|value| !value.is_empty()) {
            Some(explicit) => document.get(explicit),
            None => document
                .effective_default()
                .and_then(|default| document.get(default)),
        };
        config.model = model;
        config.context_window_tokens = target.and_then(|entry| {
            entry
                .models
                .iter()
                .flatten()
                .find(|item| item.id == config.model)
                .and_then(|item| item.context_window_tokens)
        });
        Some(config)
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

    /// List models exposed by a saved provider or an unsaved settings draft.
    ///
    /// Parameter: `request` identifies a persisted provider or supplies the
    /// connection values currently in the form. Returns a normalized catalog or
    /// a structured validation/transport error.
    ///
    /// Catalog metadata is never persisted by this method: the caller must
    /// explicitly choose a returned model and save the provider entry. That
    /// prevents a transient remote catalog response from silently changing a
    /// running configuration.
    pub async fn models(
        &self,
        request: ProviderModelCatalogRequest,
    ) -> Result<ProviderModelCatalogResponse> {
        let config = self.config_for_catalog(request)?;
        self.catalog.list(&config).await
    }

    /// Resolve the catalog connection without requiring a model id.
    fn config_for_catalog(&self, request: ProviderModelCatalogRequest) -> Result<ProviderConfig> {
        let id = request
            .provider_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let stored =
            match id {
                Some(id) => self.lock().get(id).cloned().ok_or_else(|| {
                    ArkError::not_found(format!("provider `{id}` does not exist"))
                })?,
                None => ProviderEntry::new("catalog-draft", "catalog"),
            };
        match request.connection {
            None if id.is_some() => catalog_config_from_connection(
                ProviderModelCatalogConnection {
                    base_url: stored.base_url.clone(),
                    api_key: None,
                    endpoint_format: stored.endpoint_format.clone(),
                    headers: Default::default(),
                },
                &stored,
            ),
            None => Err(ArkError::validation(
                "send a saved provider_id or a draft catalog connection",
            )),
            Some(connection) => catalog_config_from_connection(connection, &stored),
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
            let resolved = guard
                .effective_default()
                .and_then(|id| guard.get(id))
                .and_then(config_from_entry);
            ProviderListResponse {
                providers,
                default_provider_id,
                default_model: None,
                active_provider_id: resolved.as_ref().map(|config| config.provider_id.clone()),
                compaction_threshold_tokens: resolved
                    .as_ref()
                    .and_then(ProviderConfig::compaction_threshold),
                context_window_tokens: resolved
                    .as_ref()
                    .map(|config| config.effective_context_window()),
            }
        };
        Ok(list)
    }
}

/// Build a catalog-only config from a draft, preserving write-only values from
/// the stored entry when the edit form left them blank.
fn catalog_config_from_connection(
    connection: ProviderModelCatalogConnection,
    stored: &ProviderEntry,
) -> Result<ProviderConfig> {
    let base_url = validate_base_url(&connection.base_url)?;
    let endpoint_format = validate_endpoint_format(&connection.endpoint_format)?;
    let api_key = connection
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| stored.api_key.clone())
        .ok_or_else(|| ArkError::validation("api_key is required before querying models"))?;
    let headers = if connection.headers.is_empty() {
        stored.headers.clone()
    } else {
        connection.headers
    };
    let mut config = ProviderConfig::new(&stored.id, "catalog")
        .with_base_url(base_url)
        .with_api_key(api_key)
        .with_format(endpoint_format)
        .with_headers(headers);
    config.context_window_tokens = None;
    Ok(config)
}

/// The current settings form does not receive secret or advanced values back
/// from the redacted list response. Empty/absent values therefore mean "not
/// edited", not "erase the persisted value". A later explicit patch shape
/// will carry a separate clear operation instead of making absence ambiguous.
///
/// `models` participates in the retention: it is `Option<Vec<_>>` on the wire,
/// so an omitted key keeps the stored inventory (partial-edit clients such as
/// the MCP `provider_upsert` tool never send it) and an explicit list —
/// including an empty one — replaces it.
fn keep_omitted_edit_fields(
    mut candidate: ProviderEntry,
    previous: Option<&ProviderEntry>,
) -> ProviderEntry {
    let Some(previous) = previous else {
        return candidate;
    };
    if candidate.api_key.is_none() {
        candidate.api_key = previous.api_key.clone();
    }
    if candidate.headers.is_empty() {
        candidate.headers = previous.headers.clone();
    }
    if candidate.max_tokens.is_none() {
        candidate.max_tokens = previous.max_tokens;
    }
    if candidate.temperature.is_none() {
        candidate.temperature = previous.temperature;
    }
    if candidate.context_window_tokens.is_none() {
        candidate.context_window_tokens = previous.context_window_tokens;
    }
    if candidate.supports_vision.is_none() {
        candidate.supports_vision = previous.supports_vision;
    }
    if candidate.models.is_none() {
        candidate.models = previous.models.clone();
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
    // The suite lives in its own file (AGENTS §4 file-size limit); declared as
    // a descendant so the suite still sees this module through `super::*`.
    // `#[path]` resolves against the module's directory (`src/service/store/`).
    #[path = "store_tests.rs"]
    mod store_tests;
}
