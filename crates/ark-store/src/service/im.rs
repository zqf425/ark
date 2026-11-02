//! JSON-backed IM channel store.
//!
//! One document (`<data_dir>/im.json`) holds every channel record (channel +
//! inbound cursor) and every chat→thread binding. That shape mirrors
//! `schedules.json`: user-authored configuration that must survive a restart on
//! its own, with low write frequency (bindings change once per new
//! conversation, cursors are throttled by the caller).
//!
//! ## Write model
//!
//! Reads and rewrites are serialized by one in-process `Mutex`, and every
//! rewrite goes through [`write_json_atomic`] (temp file, fsync, rename). The
//! in-memory snapshot only advances after the write succeeds. The one
//! multi-entity rule is channel deletion: it removes the channel and its
//! bindings in the same locked critical section, because an orphaned binding
//! would route an inbound message into a thread via a channel that no longer
//! exists.
//!
//! ## Recovery
//!
//! A missing document is an empty table. A *present but unreadable* one is also
//! folded to an empty table, with a warning — refusing to start over a stray
//! byte would take the runtime down for settings the user can re-enter, but the
//! loss must be visible.
//!
//! ## What this layer does not do
//!
//! No credential validation, no channel cap, no merge semantics: those live in
//! `ark-im` and the server orchestration. Persisting exactly what it is handed
//! is what lets the HTTP layer return the same document it just wrote.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use ark_common::{read_json_file_opt, ArkError, Result};
use ark_contract::{ImBinding, ImChannelId};
use ark_ports::{ImChannelRecord, ImStore};
use serde::{Deserialize, Serialize};

use crate::store::write_json_atomic;

/// Document name inside the data directory.
pub const IM_FILE_NAME: &str = "im.json";

/// On-disk document. Wrapper structs (rather than bare `Vec`s) so future
/// fields can be added without changing the file's top-level shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ImFile {
    #[serde(default)]
    channels: Vec<ImChannelRecord>,
    #[serde(default)]
    bindings: Vec<ImBinding>,
}

/// File-backed implementation of [`ImStore`].
pub struct ImFileStore {
    path: PathBuf,
    state: Mutex<ImFile>,
}

impl ImFileStore {
    /// Open `<data_dir>/im.json`, treating absent or damaged content as an
    /// empty table.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(IM_FILE_NAME);
        let state = match read_json_file_opt::<ImFile>(&path) {
            Some(state) => state,
            None => {
                if path.exists() {
                    tracing::warn!(
                        path = %path.display(),
                        "im.json is unreadable; starting from an empty channel table"
                    );
                }
                ImFile::default()
            }
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    /// Path of the backing document, for diagnostics and tests.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> MutexGuard<'_, ImFile> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Copy-on-write: the caller mutates the clone, persists it, and only then
    /// swaps it in.
    fn snapshot(state: &ImFile) -> ImFile {
        ImFile {
            channels: state.channels.clone(),
            bindings: state.bindings.clone(),
        }
    }

    fn persist(&self, next: &ImFile) -> Result<()> {
        write_json_atomic(&self.path, next)
    }

    fn missing(id: &ImChannelId) -> ArkError {
        ArkError::not_found(format!("im channel not found: {id}"))
    }
}

impl ImStore for ImFileStore {
    fn list_channels(&self) -> Result<Vec<ImChannelRecord>> {
        let state = self.lock();
        let mut channels = state.channels.clone();
        // Newest edit first, matching the schedules list semantics.
        channels.sort_by_key(|record| std::cmp::Reverse(record.channel.updated_at));
        Ok(channels)
    }

    fn get_channel(&self, id: &ImChannelId) -> Result<ImChannelRecord> {
        self.lock()
            .channels
            .iter()
            .find(|record| record.channel.id == *id)
            .cloned()
            .ok_or_else(|| Self::missing(id))
    }

    fn insert_channel(&self, mut record: ImChannelRecord) -> Result<ImChannelRecord> {
        let mut state = self.lock();
        if record.channel.id.0.trim().is_empty() {
            record.channel.id = ImChannelId::new();
        }
        if state
            .channels
            .iter()
            .any(|existing| existing.channel.id == record.channel.id)
        {
            // `insert` is the create path, so a duplicate id is a bug in the
            // caller (or a retried request), not a request to overwrite.
            return Err(ArkError::conflict(format!(
                "im channel already exists: {}",
                record.channel.id
            )));
        }
        let mut next = Self::snapshot(&state);
        next.channels.push(record.clone());
        self.persist(&next)?;
        *state = next;
        Ok(record)
    }

    fn replace_channel(&self, record: ImChannelRecord) -> Result<ImChannelRecord> {
        let mut state = self.lock();
        let index = state
            .channels
            .iter()
            .position(|existing| existing.channel.id == record.channel.id)
            .ok_or_else(|| Self::missing(&record.channel.id))?;
        let mut next = Self::snapshot(&state);
        next.channels[index] = record.clone();
        self.persist(&next)?;
        *state = next;
        Ok(record)
    }

    fn delete_channel(&self, id: &ImChannelId) -> Result<()> {
        let mut state = self.lock();
        let index = state
            .channels
            .iter()
            .position(|record| record.channel.id == *id)
            .ok_or_else(|| Self::missing(id))?;
        let mut next = Self::snapshot(&state);
        next.channels.remove(index);
        // Cascade in the same critical section: a binding whose channel is
        // gone can never be resolved again, and leaving one behind would make
        // the next inbound message on that chat hit a phantom route.
        next.bindings.retain(|binding| binding.channel_id != *id);
        self.persist(&next)?;
        *state = next;
        Ok(())
    }

    fn list_bindings(&self, channel_id: Option<&ImChannelId>) -> Result<Vec<ImBinding>> {
        let state = self.lock();
        let mut bindings: Vec<ImBinding> = state
            .bindings
            .iter()
            .filter(|binding| channel_id.is_none_or(|id| binding.channel_id == *id))
            .cloned()
            .collect();
        bindings.sort_by_key(|binding| std::cmp::Reverse(binding.updated_at));
        Ok(bindings)
    }

    fn get_binding(&self, channel_id: &ImChannelId, chat_id: &str) -> Result<Option<ImBinding>> {
        Ok(self
            .lock()
            .bindings
            .iter()
            .find(|binding| binding.channel_id == *channel_id && binding.chat_id == chat_id)
            .cloned())
    }

    fn set_binding(&self, binding: ImBinding) -> Result<()> {
        let mut state = self.lock();
        let mut next = Self::snapshot(&state);
        match next.bindings.iter_mut().find(|existing| {
            existing.channel_id == binding.channel_id && existing.chat_id == binding.chat_id
        }) {
            Some(existing) => *existing = binding,
            None => next.bindings.push(binding),
        }
        self.persist(&next)?;
        *state = next;
        Ok(())
    }

    fn delete_binding(&self, channel_id: &ImChannelId, chat_id: &str) -> Result<()> {
        let mut state = self.lock();
        let mut next = Self::snapshot(&state);
        next.bindings
            .retain(|binding| !(binding.channel_id == *channel_id && binding.chat_id == chat_id));
        self.persist(&next)?;
        *state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_contract::{ImChannel, ImChannelKind, ImChannelPolicy, ImCredentials};
    use chrono::{TimeZone, Utc};

    fn channel(id: &str, title: &str) -> ImChannel {
        let now = Utc.with_ymd_and_hms(2026, 11, 1, 9, 0, 0).unwrap();
        ImChannel {
            id: ImChannelId(id.to_string()),
            kind: ImChannelKind::Telegram,
            title: title.to_string(),
            enabled: true,
            credentials: Some(ImCredentials::Telegram {
                bot_token: "tok".into(),
                bot_username: None,
            }),
            policy: ImChannelPolicy::default(),
            workspace: None,
            model: None,
            provider_id: None,
            autonomy: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn record(id: &str, title: &str) -> ImChannelRecord {
        ImChannelRecord {
            channel: channel(id, title),
            cursor: None,
        }
    }

    fn store() -> (tempfile::TempDir, ImFileStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ImFileStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn insert_assigns_an_id_when_the_caller_leaves_it_empty() {
        let (_dir, store) = store();
        let mut record = record("", "tg");
        record.channel.id = ImChannelId(String::new());
        let stored = store.insert_channel(record).unwrap();
        assert!(
            stored.channel.id.0.starts_with("im_"),
            "{}",
            stored.channel.id
        );
        assert_eq!(
            store.get_channel(&stored.channel.id).unwrap().channel.title,
            "tg"
        );
    }

    #[test]
    fn a_duplicate_insert_is_a_conflict_not_an_overwrite() {
        let (_dir, store) = store();
        store.insert_channel(record("im_1", "first")).unwrap();
        let err = store.insert_channel(record("im_1", "second")).unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::Conflict);
        assert_eq!(
            store
                .get_channel(&ImChannelId("im_1".into()))
                .unwrap()
                .channel
                .title,
            "first"
        );
    }

    #[test]
    fn replace_persists_the_cursor() {
        let (_dir, store) = store();
        store.insert_channel(record("im_1", "tg")).unwrap();
        let mut stored = store.get_channel(&ImChannelId("im_1".into())).unwrap();
        stored.cursor = Some("4200".into());
        store.replace_channel(stored).unwrap();

        let reloaded = store.get_channel(&ImChannelId("im_1".into())).unwrap();
        assert_eq!(reloaded.cursor.as_deref(), Some("4200"));
    }

    #[test]
    fn delete_channel_cascades_its_bindings() {
        let (_dir, store) = store();
        store.insert_channel(record("im_1", "tg")).unwrap();
        store.insert_channel(record("im_2", "fs")).unwrap();
        let thread = ark_common::ThreadId::new();
        store
            .set_binding(ark_ports::binding_now(
                ImChannelId("im_1".into()),
                "100",
                thread,
            ))
            .unwrap();
        store
            .set_binding(ark_ports::binding_now(
                ImChannelId("im_2".into()),
                "200",
                thread,
            ))
            .unwrap();

        store.delete_channel(&ImChannelId("im_1".into())).unwrap();

        assert!(store
            .get_binding(&ImChannelId("im_1".into()), "100")
            .unwrap()
            .is_none());
        assert!(store
            .get_binding(&ImChannelId("im_2".into()), "200")
            .unwrap()
            .is_some());
    }

    #[test]
    fn set_binding_upserts_and_list_filters_by_channel() {
        let (_dir, store) = store();
        let thread = ark_common::ThreadId::new();
        store
            .set_binding(ark_ports::binding_now(
                ImChannelId("im_1".into()),
                "100",
                thread,
            ))
            .unwrap();
        store
            .set_binding(ark_ports::binding_now(
                ImChannelId("im_1".into()),
                "100",
                ark_common::ThreadId::new(),
            ))
            .unwrap();

        let all = store
            .list_bindings(Some(&ImChannelId("im_1".into())))
            .unwrap();
        assert_eq!(all.len(), 1);
        assert!(!store.list_bindings(None).unwrap().is_empty());
        assert_ne!(all[0].thread_id, thread);
    }

    #[test]
    fn deleting_an_absent_binding_is_not_an_error() {
        let (_dir, store) = store();
        store
            .delete_binding(&ImChannelId("im_x".into()), "1")
            .unwrap();
    }

    #[test]
    fn replacing_or_getting_an_absent_channel_is_not_found() {
        let (_dir, store) = store();
        let err = store.replace_channel(record("im_absent", "x")).unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
        let err = store
            .get_channel(&ImChannelId("im_absent".into()))
            .unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
        let err = store
            .delete_channel(&ImChannelId("im_absent".into()))
            .unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
    }

    #[test]
    fn a_damaged_document_falls_back_to_an_empty_table() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(IM_FILE_NAME), b"{ not json").unwrap();
        let store = ImFileStore::open(dir.path()).unwrap();
        assert!(store.list_channels().unwrap().is_empty());
    }

    #[test]
    fn a_reopened_store_sees_the_persisted_channels_and_bindings() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = ImFileStore::open(dir.path()).unwrap();
            store.insert_channel(record("im_1", "tg")).unwrap();
            store
                .set_binding(ark_ports::binding_now(
                    ImChannelId("im_1".into()),
                    "100",
                    ark_common::ThreadId::new(),
                ))
                .unwrap();
        }
        let reopened = ImFileStore::open(dir.path()).unwrap();
        assert_eq!(reopened.list_channels().unwrap().len(), 1);
        assert_eq!(reopened.list_bindings(None).unwrap().len(), 1);
    }
}
