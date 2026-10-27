//! Scoped platform-service handles (design 7.5.5).
//!
//! Every handle is constructed by the host with a `plugin_id` baked in. The
//! plugin API has no parameter for an owner, so a plugin cannot widen its
//! namespace even if it knows another plugin's id:
//!
//! * config keys are physically `{plugin_id}.{key}`;
//! * KV keys are prefixed with the owner inside one shared map;
//! * files resolve under `data/plugins/<plugin_id>/` only;
//! * credentials are qualified with the owner before they reach the vault;
//! * log records always carry `plugin = <plugin_id>`, overriding any attempt to
//!   set it.

use ark_common::{ArkError, Result};
use ark_ports::services::LogRecord;
use ark_ports::{
    ConfigView, CredentialService, FileService, KvService, LogService, MetricService,
    NotifyService, TraceService,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Validate a plugin-local key (`route.*`, `last_thread_id`).
fn validate_local_key(key: &str) -> Result<&str> {
    let key = key.trim();
    let ok_len = !key.is_empty() && key.len() <= 200;
    let ok_chars = key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '/' | '*'));
    let ok_shape = !key.starts_with('.') && !key.ends_with('.') && !key.contains("..");
    if ok_len && ok_chars && ok_shape {
        Ok(key)
    } else {
        Err(ArkError::validation(format!("invalid plugin key {key:?}"))
            .with_detail("plugin_key_invalid"))
    }
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

/// Config view bound to one plugin. Storage stays in one map keyed by
/// `{plugin_id}.{key}`, which is also what the config file on disk uses.
#[derive(Debug)]
pub struct ScopedConfig {
    owner: String,
    store: Arc<Mutex<BTreeMap<String, serde_json::Value>>>,
}

impl ScopedConfig {
    pub fn new(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            store: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn with_store(
        owner: impl Into<String>,
        store: Arc<Mutex<BTreeMap<String, serde_json::Value>>>,
    ) -> Self {
        Self {
            owner: owner.into(),
            store,
        }
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Physical key for a logical key; exposed for tests and diagnostics.
    pub fn physical_key(&self, key: &str) -> Result<String> {
        Ok(format!("{}.{}", self.owner, validate_local_key(key)?))
    }
}

impl ConfigView for ScopedConfig {
    fn get(&self, key: &str) -> Option<serde_json::Value> {
        let physical = self.physical_key(key).ok()?;
        self.store.lock().ok()?.get(&physical).cloned()
    }

    fn set(&self, key: &str, value: serde_json::Value) -> Result<()> {
        let physical = self.physical_key(key)?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| ArkError::internal("config store poisoned"))?;
        store.insert(physical, value);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// kv
// ---------------------------------------------------------------------------

/// KV view bound to one plugin. Keys are prefixed internally.
#[derive(Debug)]
pub struct ScopedKv {
    owner: String,
    store: Arc<Mutex<BTreeMap<String, String>>>,
}

impl ScopedKv {
    pub fn new(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            store: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn with_store(
        owner: impl Into<String>,
        store: Arc<Mutex<BTreeMap<String, String>>>,
    ) -> Self {
        Self {
            owner: owner.into(),
            store,
        }
    }

    fn physical(&self, key: &str) -> Result<String> {
        Ok(format!("{}::{}", self.owner, validate_local_key(key)?))
    }

    /// Keys this plugin owns, without the namespace prefix.
    pub fn keys(&self) -> BTreeSet<String> {
        let prefix = format!("{}::", self.owner);
        self.store
            .lock()
            .map(|store| {
                store
                    .keys()
                    .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl KvService for ScopedKv {
    fn get(&self, key: &str) -> Option<String> {
        let physical = self.physical(key).ok()?;
        self.store.lock().ok()?.get(&physical).cloned()
    }

    fn put(&self, key: &str, value: &str) -> Result<()> {
        let physical = self.physical(key)?;
        self.store
            .lock()
            .map_err(|_| ArkError::internal("kv store poisoned"))?
            .insert(physical, value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let physical = self.physical(key)?;
        self.store
            .lock()
            .map_err(|_| ArkError::internal("kv store poisoned"))?
            .remove(&physical);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// files
// ---------------------------------------------------------------------------

/// File view rooted at `data/plugins/<plugin_id>/`.
#[derive(Debug)]
pub struct ScopedFiles {
    owner: String,
    root: PathBuf,
}

impl ScopedFiles {
    /// `base` is `data/plugins`; the owner subtree is appended, so a plugin can
    /// never point the root at another plugin's directory.
    pub fn under(base: impl AsRef<Path>, owner: impl Into<String>) -> Self {
        let owner = owner.into();
        let root = base.as_ref().join(&owner);
        Self { owner, root }
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl FileService for ScopedFiles {
    fn read_relative(&self, rel: &str) -> Result<Vec<u8>> {
        let path = crate::domain::resolve_within_root(&self.root, rel)?;
        std::fs::read(&path).map_err(|err| {
            ArkError::store(format!("read {} failed: {err}", path.display()))
                .with_detail("plugin_file_unavailable")
        })
    }

    fn write_relative(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        // The plugin root may not exist yet on first write. Create it before
        // containment is checked, because the check canonicalizes the root.
        std::fs::create_dir_all(&self.root).map_err(|err| {
            ArkError::store(format!("create {} failed: {err}", self.root.display()))
                .with_detail("plugin_root_unavailable")
        })?;
        let path = crate::domain::resolve_within_root(&self.root, rel)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes).map_err(|err| {
            ArkError::store(format!("write {} failed: {err}", path.display()))
                .with_detail("plugin_file_unavailable")
        })
    }
}

// ---------------------------------------------------------------------------
// credentials
// ---------------------------------------------------------------------------

/// Credential handle: the plugin names a secret, the host qualifies the owner.
#[derive(Debug)]
pub struct ScopedCredentials {
    owner: String,
    vault: Arc<ark_http::CredentialVault>,
}

impl ScopedCredentials {
    pub fn new(owner: impl Into<String>, vault: Arc<ark_http::CredentialVault>) -> Self {
        Self {
            owner: owner.into(),
            vault,
        }
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Host-side provisioning helper (the plugin never sees the value).
    pub fn provision(&self, name: &str, secret: &str) -> Result<()> {
        self.vault.store_for(&self.owner, name, secret)
    }
}

impl CredentialService for ScopedCredentials {
    fn store(&self, name: &str, secret: &str) -> Result<()> {
        self.vault.store_for(&self.owner, name, secret)
    }

    fn has(&self, name: &str) -> bool {
        self.vault.has_for(&self.owner, name)
    }

    fn inject_header(&self, name: &str) -> Result<Option<String>> {
        self.vault
            .resolve(&self.owner, name)
            .map(|(value, _)| Some(value))
    }
}

// ---------------------------------------------------------------------------
// log / trace / metrics / notify
// ---------------------------------------------------------------------------

/// Log view that stamps the owner into every record.
pub struct ScopedLog {
    owner: String,
    sink: Arc<dyn LogService>,
}

impl std::fmt::Debug for ScopedLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedLog")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl ScopedLog {
    pub fn new(owner: impl Into<String>, sink: Arc<dyn LogService>) -> Self {
        Self {
            owner: owner.into(),
            sink,
        }
    }
}

impl LogService for ScopedLog {
    fn log(&self, mut record: LogRecord) {
        // The owner always wins: a record claiming another plugin is rewritten.
        record.plugin = self.owner.clone();
        self.sink.log(record);
    }
}

/// Trace view; span names are prefixed with the owner.
pub struct ScopedTrace {
    owner: String,
    sink: Arc<dyn TraceService>,
}

impl std::fmt::Debug for ScopedTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedTrace")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl ScopedTrace {
    pub fn new(owner: impl Into<String>, sink: Arc<dyn TraceService>) -> Self {
        Self {
            owner: owner.into(),
            sink,
        }
    }
}

impl TraceService for ScopedTrace {
    fn span(&self, name: &str, fields: BTreeMap<String, String>) {
        self.sink.span(&format!("{}.{}", self.owner, name), fields);
    }
}

/// Metric view; names are prefixed with the owner so cardinality stays bounded.
pub struct ScopedMetrics {
    owner: String,
    sink: Arc<dyn MetricService>,
}

impl std::fmt::Debug for ScopedMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedMetrics")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl ScopedMetrics {
    pub fn new(owner: impl Into<String>, sink: Arc<dyn MetricService>) -> Self {
        Self {
            owner: owner.into(),
            sink,
        }
    }
}

impl MetricService for ScopedMetrics {
    fn counter(&self, name: &str, value: u64) {
        self.sink
            .counter(&format!("{}.{}", self.owner, name), value);
    }
}

/// Notification view that routes with the owner attached.
pub struct ScopedNotify {
    owner: String,
    sink: Arc<dyn NotifyService>,
}

impl std::fmt::Debug for ScopedNotify {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedNotify")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl ScopedNotify {
    pub fn new(owner: impl Into<String>, sink: Arc<dyn NotifyService>) -> Self {
        Self {
            owner: owner.into(),
            sink,
        }
    }
}

impl NotifyService for ScopedNotify {
    fn notify(&self, title: &str, body: &str) -> Result<()> {
        self.sink.notify(&format!("[{}] {title}", self.owner), body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(plugin: &str) -> LogRecord {
        LogRecord {
            level: "info".into(),
            message: "hello".into(),
            plugin: plugin.into(),
            fields: BTreeMap::new(),
        }
    }

    #[derive(Default)]
    struct CaptureLog(Mutex<Vec<LogRecord>>);

    impl LogService for CaptureLog {
        fn log(&self, record: LogRecord) {
            self.0.lock().unwrap().push(record);
        }
    }

    #[test]
    fn config_keys_are_physically_namespaced() {
        let store = Arc::new(Mutex::new(BTreeMap::new()));
        let a = ScopedConfig::with_store("ark.a", store.clone());
        let b = ScopedConfig::with_store("ark.b", store.clone());
        a.set("route.url", serde_json::json!("a")).unwrap();
        b.set("route.url", serde_json::json!("b")).unwrap();
        assert_eq!(a.get("route.url").unwrap(), serde_json::json!("a"));
        assert_eq!(b.get("route.url").unwrap(), serde_json::json!("b"));
        assert_eq!(a.physical_key("route.url").unwrap(), "ark.a.route.url");
        // No cross-read even when the foreign owner is spelled out.
        assert!(a.get("ark.b.route.url").is_none());
    }

    #[test]
    fn config_rejects_keys_that_try_to_escape() {
        let config = ScopedConfig::new("ark.a");
        for bad in ["", "..", "../x", ".hidden", "a..b"] {
            assert!(config.set(bad, serde_json::json!(1)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn kv_keys_do_not_collide_between_plugins() {
        let store = Arc::new(Mutex::new(BTreeMap::new()));
        let a = ScopedKv::with_store("ark.a", store.clone());
        let b = ScopedKv::with_store("ark.b", store.clone());
        a.put("last", "1").unwrap();
        b.put("last", "2").unwrap();
        assert_eq!(a.get("last").as_deref(), Some("1"));
        assert_eq!(b.get("last").as_deref(), Some("2"));
        // A key that names another plugin still lives in the caller's namespace.
        a.put("ark.b::last", "spoof").unwrap();
        assert_eq!(b.get("last").as_deref(), Some("2"));
        assert_eq!(a.keys().len(), 2);
        a.delete("last").unwrap();
        assert_eq!(a.get("last"), None);
    }

    #[test]
    fn files_are_rooted_under_the_owner_and_traversal_is_rejected() {
        let base = std::env::temp_dir().join(format!(
            "ark-plugin-files-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let files = ScopedFiles::under(&base, "ark.a");
        assert_eq!(files.root(), base.join("ark.a"));
        files.write_relative("notes/a.txt", b"hi").unwrap();
        assert_eq!(files.read_relative("notes/a.txt").unwrap(), b"hi");

        let err = files
            .write_relative("../ark.b/steal.txt", b"x")
            .unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("plugin_path_traversal"));
        assert!(!base.join("ark.b/steal.txt").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn credentials_are_qualified_with_the_owner() {
        let vault = Arc::new(ark_http::CredentialVault::new());
        let a = ScopedCredentials::new("ark.a", vault.clone());
        let b = ScopedCredentials::new("ark.b", vault.clone());
        a.provision("token", "secret-a").unwrap();
        assert!(a.has("token"));
        assert!(!b.has("token"), "another plugin must not see the secret");
        assert!(b.inject_header("token").is_err());
        assert_eq!(
            a.inject_header("token").unwrap().as_deref(),
            Some("secret-a")
        );
    }

    #[test]
    fn log_plugin_field_cannot_be_overridden() {
        let sink = Arc::new(CaptureLog::default());
        let log = ScopedLog::new("ark.a", sink.clone());
        log.log(record("ark.b"));
        let captured = sink.0.lock().unwrap();
        assert_eq!(captured[0].plugin, "ark.a");
    }
}
