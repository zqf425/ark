//! Platform services (design §7.5.4).
//!
//! Every handle is constructed by the host and bound to a `plugin_id`, so a
//! plugin cannot address another plugin's namespace by passing an id.

use ark_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Structured log line. `plugin` and version are injected by the host and can
/// never be overridden by the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRecord {
    pub level: String,
    pub message: String,
    pub plugin: String,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

pub trait LogService: Send + Sync {
    fn log(&self, record: LogRecord);
}

/// Private config view: keys are physically namespaced as `{plugin_id}.{key}`.
pub trait ConfigView: Send + Sync {
    fn get(&self, key: &str) -> Option<serde_json::Value>;

    fn set(&self, key: &str, value: serde_json::Value) -> Result<()>;
}

/// Credentials are write-once opaque handles: no plaintext read API.
pub trait CredentialService: Send + Sync {
    fn store(&self, name: &str, secret: &str) -> Result<()>;

    fn has(&self, name: &str) -> bool;

    /// Returns a header value for injection into an outbound request.
    fn inject_header(&self, name: &str) -> Result<Option<String>>;
}

/// Small private state, key space fixed to the owning plugin.
pub trait KvService: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;

    fn put(&self, key: &str, value: &str) -> Result<()>;

    fn delete(&self, key: &str) -> Result<()>;
}

/// Private files rooted at `data/plugins/<plugin_id>/`; traversal is rejected.
pub trait FileService: Send + Sync {
    fn read_relative(&self, rel: &str) -> Result<Vec<u8>>;

    fn write_relative(&self, rel: &str, bytes: &[u8]) -> Result<()>;
}

pub trait TraceService: Send + Sync {
    fn span(&self, name: &str, fields: BTreeMap<String, String>);
}

pub trait MetricService: Send + Sync {
    fn counter(&self, name: &str, value: u64);
}

pub trait NotifyService: Send + Sync {
    fn notify(&self, title: &str, body: &str) -> Result<()>;
}
