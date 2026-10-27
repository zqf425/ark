//! Persistence for configured external MCP servers.
//!
//! One JSON file, `<data_dir>/mcp-servers.json`, written through
//! [`ark_common::write_json_file`] so the bytes are UTF-8 without a BOM and the
//! rename is atomic. A BOM in front of a config document is the most common
//! Windows startup failure in this project, so every read goes through
//! [`ark_common::read_json_file_opt`], which strips it.
//!
//! ## Secrets on disk (v1 limitation)
//!
//! `env` and `headers` may contain tokens. v1 stores them in this plain JSON file
//! because there is no OS keyring integration yet. The read path never returns
//! them to the GUI (`McpServerView` has no such fields), but a later revision
//! must move the values into the credential store and keep only references here.

use ark_common::{read_json_file_opt, write_json_file, Result};
use ark_contract::McpServerEntry;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// File name used inside the data directory.
pub const FILE_NAME: &str = "mcp-servers.json";
/// Bumped only when the on-disk shape changes incompatibly.
const SCHEMA_VERSION: u32 = 1;

/// On-disk document. The wrapper leaves room for future migrations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Document {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    servers: Vec<McpServerEntry>,
}

/// Read/write handle for `<data_dir>/mcp-servers.json`.
#[derive(Debug, Clone)]
pub(crate) struct ServerStore {
    path: PathBuf,
}

impl ServerStore {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(FILE_NAME),
        }
    }

    /// Absolute path of the backing file (used by tests and diagnostics).
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Load every entry. A missing or unparsable document is an empty config,
    /// never a startup failure: a corrupt file must not brick the app.
    ///
    /// Invalid entries are dropped here instead of failing the whole load, so
    /// one hand-edited mistake cannot hide every other server.
    pub(crate) fn load(&self) -> Vec<McpServerEntry> {
        let document: Document = read_json_file_opt(&self.path).unwrap_or_default();
        document
            .servers
            .into_iter()
            .filter_map(|entry| crate::domain::server_entry::normalize(entry).ok())
            .fold(Vec::new(), |mut acc, entry| {
                // Last definition wins, mirroring upsert semantics.
                acc.retain(|existing: &McpServerEntry| existing.id != entry.id);
                acc.push(entry);
                acc
            })
    }

    /// Persist the full list. Written atomically, so a crash cannot truncate it.
    pub(crate) fn save(&self, entries: &[McpServerEntry]) -> Result<()> {
        let document = Document {
            version: SCHEMA_VERSION,
            servers: entries.to_vec(),
        };
        write_json_file(&self.path, &document)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::McpTransportKind;
    use std::collections::BTreeMap;

    fn entry(id: &str) -> McpServerEntry {
        let mut env = BTreeMap::new();
        env.insert("TOKEN".to_string(), "s3cret-value".to_string());
        McpServerEntry {
            id: id.to_string(),
            transport: McpTransportKind::Stdio,
            enabled: true,
            command: Some("npx".to_string()),
            args: vec!["-y".to_string(), "some-mcp".to_string()],
            env,
            url: None,
            headers: BTreeMap::new(),
            allow_tools: Vec::new(),
        }
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let dir = std::env::temp_dir().join(format!("ark-mcp-store-{}", std::process::id()));
        let store = ServerStore::new(&dir);
        assert!(store.load().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trip_keeps_every_field_without_bom() {
        let dir = std::env::temp_dir().join(format!("ark-mcp-store-rt-{}", std::process::id()));
        let store = ServerStore::new(&dir);
        let first = entry("jira");
        store.save(std::slice::from_ref(&first)).unwrap();

        let bytes = std::fs::read(store.path()).unwrap();
        assert_ne!(bytes.first(), Some(&0xEF), "file must not start with a BOM");

        let loaded = store.load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_entries_are_dropped_and_last_definition_wins() {
        let dir = std::env::temp_dir().join(format!("ark-mcp-store-bad-{}", std::process::id()));
        let store = ServerStore::new(&dir);
        let mut duplicate = entry("dup");
        duplicate.command = Some("node".to_string());
        store
            .save(&[entry("ok"), entry("bad id"), duplicate, entry("dup")])
            .unwrap();

        let loaded = store.load();
        let ids: Vec<&str> = loaded.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["ok", "dup"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
