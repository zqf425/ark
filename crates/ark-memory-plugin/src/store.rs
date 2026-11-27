//! Memory records and their on-disk layout.
//!
//! Files are the only source of truth: the KV index is a cache rebuilt from
//! disk at startup, mirroring the `ark-store` rule that a projection may be
//! discarded but a log may not. This module is pure apart from the two calls
//! that touch `FileService`/`KvService`, so the record shape and the index
//! folding are testable without a temp directory.

use std::collections::BTreeSet;

use ark_common::{ArkError, Result};
use ark_ports::{FileService, KvService};
use serde::{Deserialize, Serialize};

/// Directory (relative to the plugin root) holding one JSON per memory.
const MEMORY_DIR: &str = "memories";

/// KV key holding the newline-joined id list. Kept small on purpose: the
/// recall path reads every record anyway when the query has no useful token.
const INDEX_KEY: &str = "index";

/// File holding the same id list, so the index survives a process restart.
///
/// `ark_ports::FileService` can read and write a named path but cannot enumerate
/// a directory, and `ScopedKv` is an in-memory map. Without this mirror a fresh
/// process would see an empty index and report "no memories" for a plugin whose
/// files are all still on disk. Writing the list to a file we name ourselves is
/// the only way to rediscover ids through the open port surface; the manifest
/// file is rewritten on every mutation, so a crash costs at most one entry.
const MANIFEST_FILE: &str = "memories.index";

/// Hard cap on one memory's text, in characters.
///
/// Matches the host's own long-term memory budget so a plugin memory can be
/// promoted into it without truncation surprises.
pub const MAX_TEXT_CHARS: usize = 16 * 1024;

/// One stored memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// RFC 3339 UTC. Ordered lexicographically, which is why `list` can sort
    /// strings instead of parsing timestamps.
    pub created_at: String,
    /// Tombstone flag. `ark_ports::FileService` exposes only read/write, so a
    /// delete rewrites the file instead of unlinking it; every read path
    /// filters tombstones out. See the module doc for why this is preferred
    /// over widening the port.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deleted: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// File-backed memory store bound to one plugin's scoped handles.
pub struct MemoryStore {
    files: std::sync::Arc<dyn FileService>,
    kv: std::sync::Arc<dyn KvService>,
}

impl MemoryStore {
    pub fn new(files: std::sync::Arc<dyn FileService>, kv: std::sync::Arc<dyn KvService>) -> Self {
        Self { files, kv }
    }

    /// Store one memory. Empty text is rejected rather than written: an entry
    /// the model cannot read back is noise in every later search.
    pub fn store(
        &self,
        id: &str,
        text: &str,
        tags: &[String],
        source: Option<&str>,
        created_at: &str,
    ) -> Result<MemoryRecord> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(ArkError::validation("memory text is empty"));
        }
        let chars = trimmed.chars().count();
        if chars > MAX_TEXT_CHARS {
            return Err(ArkError::validation(format!(
                "memory text is {chars} characters; the limit is {MAX_TEXT_CHARS}"
            )));
        }
        let record = MemoryRecord {
            id: validate_id(id)?.to_string(),
            text: trimmed.to_string(),
            tags: normalise_tags(tags),
            source: source.map(str::to_string).filter(|s| !s.is_empty()),
            created_at: created_at.to_string(),
            deleted: false,
        };
        // File first: the KV index must never name a record that is not durable.
        self.files
            .write_relative(&record_path(&record.id), record_body(&record)?.as_bytes())?;
        self.index_add(&record.id)?;
        Ok(record)
    }

    /// Every readable memory, newest first.
    ///
    /// An unparseable file is skipped rather than failing the whole list: one
    /// damaged entry must not hide the rest, and the damage is visible as a
    /// missing id on the next `store`.
    pub fn list(&self) -> Vec<MemoryRecord> {
        let mut records = self.all_records();
        records.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        records
    }

    /// Keyword search over text and tags, newest first, capped at `limit`.
    ///
    /// Deliberately lexical: the plugin has no embedding model, and a remote
    /// one would make local memory require the network.
    pub fn search(&self, query: &str, tags: &[String], limit: usize) -> Vec<MemoryRecord> {
        let needle = query.trim().to_lowercase();
        let wanted = normalise_tags(tags);
        let mut hits: Vec<MemoryRecord> = self
            .all_records()
            .into_iter()
            .filter(|record| matches_tags(record, &wanted))
            .filter(|record| needle.is_empty() || matches_text(record, &needle))
            .collect();
        hits.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        hits.truncate(limit);
        hits
    }

    /// Remove one memory by writing a tombstone.
    ///
    /// Reports `false` when the id was absent or already deleted, so the tool
    /// answers honestly instead of claiming a delete that never happened.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let id = validate_id(id)?;
        let mut record = match self.read(id) {
            Ok(record) if !record.deleted => record,
            // Absent or already tombstoned: the postcondition ("this id is
            // gone") already holds, so this is idempotent rather than an error.
            _ => {
                self.index_remove(id)?;
                return Ok(false);
            }
        };
        record.deleted = true;
        // Tombstone first, then drop the index entry: a crash between the two
        // leaves a readable record marked deleted, which the read paths skip.
        self.files
            .write_relative(&record_path(&record.id), record_body(&record)?.as_bytes())?;
        self.index_remove(id)?;
        Ok(true)
    }

    /// Record ids currently indexed.
    ///
    /// The file mirror is read on a KV miss, which is what makes a restarted
    /// process see the memories a previous one stored.
    pub fn ids(&self) -> Vec<String> {
        if let Some(raw) = self.kv.get(INDEX_KEY) {
            return parse_index(&raw);
        }
        match self.files.read_relative(MANIFEST_FILE) {
            Ok(bytes) => {
                let ids = parse_index(&String::from_utf8_lossy(&bytes));
                // Warm the cache from the mirror so later reads skip the disk.
                let _ = self.kv.put(INDEX_KEY, &ids.join("\n"));
                ids
            }
            // Absent mirror is the first-run case, not an error.
            Err(_) => Vec::new(),
        }
    }

    /// Rebuild the index from the records the mirror still names.
    ///
    /// Ids can only come from the mirror: `FileService` has no directory
    /// enumeration, so records whose ids are not in the mirror are
    /// unreachable — which is why every mutation rewrites it. Returns how many
    /// live records remain.
    pub fn rebuild_index(&self) -> Result<usize> {
        let ids: Vec<String> = self
            .all_records()
            .into_iter()
            .map(|record| record.id)
            .collect();
        self.write_index(&ids)?;
        Ok(ids.len())
    }

    /// Persist the id list to the mirror, then to the KV cache.
    ///
    /// Mirror first: a crash between the two leaves a KV cache that is behind
    /// the files, and the next `ids()` re-warms from the mirror.
    fn write_index(&self, ids: &[String]) -> Result<()> {
        let body = ids.join("\n");
        self.files.write_relative(MANIFEST_FILE, body.as_bytes())?;
        self.kv.put(INDEX_KEY, &body)?;
        Ok(())
    }

    fn all_records(&self) -> Vec<MemoryRecord> {
        self.ids()
            .into_iter()
            .filter_map(|id| self.read(&id).ok())
            .filter(|record| !record.deleted)
            .collect()
    }

    fn read(&self, id: &str) -> Result<MemoryRecord> {
        let bytes = self.files.read_relative(&record_path(id))?;
        let raw = std::str::from_utf8(&bytes)
            .map_err(|_| ArkError::store(format!("memory {id} is not UTF-8")))?;
        serde_json::from_str(raw)
            .map_err(|err| ArkError::store(format!("memory {id} is not valid JSON: {err}")))
    }

    fn index_add(&self, id: &str) -> Result<()> {
        let mut ids = self.ids();
        if !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_string());
            self.write_index(&ids)?;
        }
        Ok(())
    }

    fn index_remove(&self, id: &str) -> Result<()> {
        let ids: Vec<String> = self
            .ids()
            .into_iter()
            .filter(|existing| existing != id)
            .collect();
        self.write_index(&ids)?;
        Ok(())
    }
}

/// An id becomes a file name, so it may only contain characters that cannot
/// leave the memory directory or confuse a path separator.
pub fn validate_id(id: &str) -> Result<&str> {
    let id = id.trim();
    if id.is_empty() {
        return Err(ArkError::validation("memory id is empty"));
    }
    let ok = id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if !ok {
        return Err(ArkError::validation(format!("invalid memory id {id:?}")));
    }
    Ok(id)
}

fn record_path(id: &str) -> String {
    format!("{MEMORY_DIR}/{id}.json")
}

fn record_body(record: &MemoryRecord) -> Result<String> {
    serde_json::to_string_pretty(record)
        .map_err(|err| ArkError::internal(format!("cannot encode memory: {err}")))
}

/// Tags are deduplicated and trimmed so a search filter compares stable values.
fn normalise_tags(tags: &[String]) -> Vec<String> {
    let unique: BTreeSet<String> = tags
        .iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    unique.into_iter().collect()
}

fn parse_index(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn matches_tags(record: &MemoryRecord, wanted: &[String]) -> bool {
    wanted
        .iter()
        .all(|tag| record.tags.iter().any(|own| own == tag))
}

fn matches_text(record: &MemoryRecord, needle: &str) -> bool {
    record.text.to_lowercase().contains(needle)
        || record.tags.iter().any(|tag| tag.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_plugin::service::{ScopedFiles, ScopedKv};
    use std::sync::Arc;

    fn store_in(dir: &std::path::Path) -> MemoryStore {
        let files = Arc::new(ScopedFiles::under(dir, "ark.memory.core"));
        let kv = Arc::new(ScopedKv::new("ark.memory.core"));
        MemoryStore::new(files, kv)
    }

    fn at(minute: u32) -> String {
        format!("2026-11-19T09:{minute:02}:00Z")
    }

    #[test]
    fn stores_and_reads_back_one_memory() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        store
            .store(
                "mem_1",
                "prefers short answers",
                &["pref".into()],
                Some("manual"),
                &at(1),
            )
            .unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].text, "prefers short answers");
        assert_eq!(listed[0].tags, vec!["pref"]);
    }

    #[test]
    fn empty_and_oversized_text_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        assert!(store.store("mem_1", "   ", &[], None, &at(1)).is_err());
        let huge = "x".repeat(MAX_TEXT_CHARS + 1);
        assert!(store.store("mem_2", &huge, &[], None, &at(1)).is_err());
        assert!(store.list().is_empty());
    }

    #[test]
    fn an_id_that_could_escape_the_directory_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        for bad in ["../escape", "a/b", "a\\b", ""] {
            assert!(
                store.store(bad, "text", &[], None, &at(1)).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn search_matches_text_and_filters_by_tag() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        store
            .store(
                "mem_1",
                "the user writes in Chinese",
                &["language".into()],
                None,
                &at(1),
            )
            .unwrap();
        store
            .store(
                "mem_2",
                "the user prefers tabs",
                &["style".into()],
                None,
                &at(2),
            )
            .unwrap();

        let by_text = store.search("chinese", &[], 10);
        assert_eq!(by_text.len(), 1);
        assert_eq!(by_text[0].id, "mem_1");

        let by_tag = store.search("", &["style".into()], 10);
        assert_eq!(by_tag.len(), 1);
        assert_eq!(by_tag[0].id, "mem_2");

        // Both filters together must both hold.
        assert!(store.search("chinese", &["style".into()], 10).is_empty());
    }

    #[test]
    fn list_and_search_return_newest_first_and_respect_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        for (index, minute) in [(1, 1u32), (2, 2), (3, 3)] {
            store
                .store(
                    &format!("mem_{index}"),
                    &format!("entry {index}"),
                    &[],
                    None,
                    &at(minute),
                )
                .unwrap();
        }
        let listed = store.list();
        assert_eq!(
            listed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["mem_3", "mem_2", "mem_1"]
        );
        assert_eq!(store.search("entry", &[], 2).len(), 2);
    }

    #[test]
    fn delete_reports_whether_the_id_existed() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        store.store("mem_1", "text", &[], None, &at(1)).unwrap();
        assert!(store.delete("mem_1").unwrap());
        assert!(!store.delete("mem_1").unwrap());
        assert!(store.list().is_empty());
    }

    #[test]
    fn a_restart_rediscovers_memories_through_the_index_mirror() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = store_in(dir.path());
            store.store("mem_1", "durable", &[], None, &at(1)).unwrap();
        }
        // A new store sees an empty KV, and `FileService` cannot enumerate the
        // memories directory, so the id list has to come from the mirror file.
        let restarted = store_in(dir.path());
        assert_eq!(restarted.list().len(), 1);
        assert_eq!(restarted.list()[0].text, "durable");
    }

    #[test]
    fn rebuild_drops_ids_whose_record_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        store.store("mem_1", "kept", &[], None, &at(1)).unwrap();
        store.store("mem_2", "damaged", &[], None, &at(2)).unwrap();
        let path = dir
            .path()
            .join("ark.memory.core")
            .join("memories")
            .join("mem_2.json");
        std::fs::write(path, b"not json").unwrap();

        // `rebuild_index` keeps only what it can still read.
        assert_eq!(store.rebuild_index().unwrap(), 1);
        let restarted = store_in(dir.path());
        assert_eq!(restarted.list().len(), 1);
        assert_eq!(restarted.list()[0].id, "mem_1");
    }

    #[test]
    fn a_damaged_file_is_skipped_instead_of_hiding_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        store.store("mem_1", "good", &[], None, &at(1)).unwrap();
        store
            .store("mem_2", "also good", &[], None, &at(2))
            .unwrap();
        let path = dir
            .path()
            .join("ark.memory.core")
            .join("memories")
            .join("mem_1.json");
        std::fs::write(path, b"not json").unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "mem_2");
    }
}
