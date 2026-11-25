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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use ark_common::{read_json_file_opt, ArkError, Result};
use ark_contract::{ImBinding, ImChannelId};
use ark_ports::{ImChannelRecord, ImInboxEntry, ImInboxId, ImStore, InboundMessage};
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
    #[serde(default)]
    inbox: Vec<ImInboxEntry>,
    #[serde(default)]
    next_cursor_version: u64,
    #[serde(default)]
    applied_cursor_versions: HashMap<ImChannelId, u64>,
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
            inbox: state.inbox.clone(),
            next_cursor_version: state.next_cursor_version,
            applied_cursor_versions: state.applied_cursor_versions.clone(),
        }
    }

    fn persist(&self, next: &ImFile) -> Result<()> {
        write_json_atomic(&self.path, next)
    }

    fn missing(id: &ImChannelId) -> ArkError {
        ArkError::not_found(format!("im channel not found: {id}"))
    }

    fn missing_inbox(id: &ImInboxId) -> ArkError {
        ArkError::not_found(format!("im inbox entry not found: {id}"))
    }

    fn advance_cursor(
        state: &mut ImFile,
        channel_id: &ImChannelId,
        next: Option<String>,
        version: u64,
    ) -> Result<()> {
        let record = state
            .channels
            .iter_mut()
            .find(|record| record.channel.id == *channel_id)
            .ok_or_else(|| Self::missing(channel_id))?;
        let applied = state
            .applied_cursor_versions
            .get(channel_id)
            .copied()
            .unwrap_or(0);
        if version > applied || (version == 0 && applied == 0) {
            if let Some(cursor) = next {
                record.cursor = Some(cursor);
            }
            state
                .applied_cursor_versions
                .insert(channel_id.clone(), version);
        }
        Ok(())
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
        next.applied_cursor_versions.remove(id);
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

    fn persist_inbound(
        &self,
        message: InboundMessage,
        next_cursor: Option<String>,
    ) -> Result<ImInboxId> {
        let mut ids = self.persist_inbound_batch(vec![message], next_cursor)?;
        Ok(ids.pop().expect("one persisted message has one id"))
    }

    fn persist_inbound_batch(
        &self,
        messages: Vec<InboundMessage>,
        next_cursor: Option<String>,
    ) -> Result<Vec<ImInboxId>> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let mut state = self.lock();
        for message in &messages {
            if !state
                .channels
                .iter()
                .any(|record| record.channel.id == message.channel_id)
            {
                return Err(Self::missing(&message.channel_id));
            }
        }
        let mut next = Self::snapshot(&state);
        next.next_cursor_version += 1;
        let cursor_version = next.next_cursor_version;
        let ids = messages
            .into_iter()
            .map(|message| {
                let id = ImInboxId::new();
                next.inbox.push(ImInboxEntry {
                    id: id.clone(),
                    message,
                    next_cursor: next_cursor.clone(),
                    queued: false,
                    cursor_version: Some(cursor_version),
                });
                id
            })
            .collect();
        self.persist(&next)?;
        *state = next;
        Ok(ids)
    }

    fn list_inbox(&self) -> Result<Vec<ImInboxEntry>> {
        Ok(self.lock().inbox.clone())
    }

    fn advance_channel_cursor(&self, channel_id: &ImChannelId, next: Option<String>) -> Result<()> {
        let mut state = self.lock();
        let mut snapshot = Self::snapshot(&state);
        snapshot.next_cursor_version += 1;
        let cursor_version = snapshot.next_cursor_version;
        Self::advance_cursor(&mut snapshot, channel_id, next, cursor_version)?;
        self.persist(&snapshot)?;
        *state = snapshot;
        Ok(())
    }

    fn queue_inbound(&self, id: &ImInboxId, next_cursor: Option<String>) -> Result<()> {
        let mut state = self.lock();
        let entry = state
            .inbox
            .iter_mut()
            .find(|entry| entry.id == *id)
            .ok_or_else(|| Self::missing_inbox(id))?;
        let channel_id = entry.message.channel_id.clone();
        let cursor_version = entry.cursor_version.unwrap_or(0);
        let mut next = Self::snapshot(&state);
        let entry = next
            .inbox
            .iter_mut()
            .find(|entry| entry.id == *id)
            .expect("snapshot retains inbox id");
        entry.queued = true;
        entry.next_cursor = next_cursor;
        let next_cursor = entry.next_cursor.clone();
        Self::advance_cursor(&mut next, &channel_id, next_cursor, cursor_version)?;
        self.persist(&next)?;
        *state = next;
        Ok(())
    }

    fn ack_inbound(&self, id: &ImInboxId) -> Result<()> {
        let mut state = self.lock();
        if !state.inbox.iter().any(|entry| entry.id == *id) {
            return Err(Self::missing_inbox(id));
        }
        let mut next = Self::snapshot(&state);
        next.inbox.retain(|entry| entry.id != *id);
        self.persist(&next)?;
        *state = next;
        Ok(())
    }

    fn discard_inbound(&self, id: &ImInboxId, next_cursor: Option<String>) -> Result<()> {
        let mut state = self.lock();
        let (channel_id, cursor_version) = state
            .inbox
            .iter()
            .find(|entry| entry.id == *id)
            .map(|entry| {
                (
                    entry.message.channel_id.clone(),
                    entry.cursor_version.unwrap_or(0),
                )
            })
            .ok_or_else(|| Self::missing_inbox(id))?;
        let mut next = Self::snapshot(&state);
        next.inbox.retain(|entry| entry.id != *id);
        Self::advance_cursor(&mut next, &channel_id, next_cursor, cursor_version)?;
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

    fn inbound(channel_id: &str, message_id: &str) -> InboundMessage {
        InboundMessage {
            channel_id: ImChannelId(channel_id.into()),
            chat_id: "100".into(),
            sender_id: "42".into(),
            sender_name: "Ann".into(),
            message_id: message_id.into(),
            text: "hello".into(),
            is_dm: true,
            is_mention: false,
        }
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
    fn rewriting_preserves_durable_inbox_entries() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = serde_json::json!([{
            "id": "in_1",
            "queued": false,
            "next_cursor": "8",
            "message": {
                "channel_id": "im_1",
                "chat_id": "100",
                "sender_id": "42",
                "sender_name": "Ann",
                "message_id": "7",
                "text": "hello",
                "is_dm": true,
                "is_mention": false
            }
        }]);
        std::fs::write(
            dir.path().join(IM_FILE_NAME),
            serde_json::json!({
                "channels": [],
                "bindings": [],
                "inbox": inbox
            })
            .to_string(),
        )
        .unwrap();
        let store = ImFileStore::open(dir.path()).unwrap();
        store
            .delete_binding(&ImChannelId("im_1".into()), "100")
            .unwrap();

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(store.path()).unwrap()).unwrap();
        assert_eq!(raw["inbox"], inbox);
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

    #[test]
    fn inbound_batch_is_all_or_nothing_and_survives_restart() {
        let (dir, store) = store();
        store.insert_channel(record("im_1", "tg")).unwrap();

        let err = store
            .persist_inbound_batch(
                vec![inbound("im_1", "7"), inbound("im_missing", "8")],
                Some("9".into()),
            )
            .unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
        assert!(store.list_inbox().unwrap().is_empty());
        assert_eq!(
            store
                .get_channel(&ImChannelId("im_1".into()))
                .unwrap()
                .cursor,
            None
        );

        let ids = store
            .persist_inbound_batch(
                vec![inbound("im_1", "7"), inbound("im_1", "8")],
                Some("9".into()),
            )
            .unwrap();
        assert_eq!(ids.len(), 2);
        drop(store);

        let store = ImFileStore::open(dir.path()).unwrap();
        let entries = store.list_inbox().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| !entry.queued));
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            ids
        );
        assert_eq!(
            store
                .get_channel(&ImChannelId("im_1".into()))
                .unwrap()
                .cursor,
            None
        );
    }

    #[test]
    fn out_of_order_admission_cannot_move_a_channel_cursor_backwards() {
        let (dir, store) = store();
        let channel_id = ImChannelId("im_1".into());
        store.insert_channel(record("im_1", "tg")).unwrap();
        let older = store
            .persist_inbound(inbound("im_1", "7"), Some("8".into()))
            .unwrap();
        let newer = store
            .persist_inbound(inbound("im_1", "8"), Some("9".into()))
            .unwrap();

        store.queue_inbound(&newer, Some("9".into())).unwrap();
        drop(store);
        let store = ImFileStore::open(dir.path()).unwrap();
        store.queue_inbound(&older, Some("8".into())).unwrap();

        assert_eq!(
            store.get_channel(&channel_id).unwrap().cursor.as_deref(),
            Some("9"),
            "a delayed older batch must not make the restarted transport poll it again"
        );
    }

    #[test]
    fn failed_atomic_batch_rewrite_preserves_memory_and_disk() {
        let (dir, store) = store();
        store.insert_channel(record("im_1", "tg")).unwrap();
        let path = store.path().to_path_buf();
        let before = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        let err = store
            .persist_inbound_batch(
                vec![inbound("im_1", "7"), inbound("im_1", "8")],
                Some("9".into()),
            )
            .unwrap_err();
        assert_eq!(err.code, ark_common::ArkErrorCode::StoreFailure);
        assert!(store.list_inbox().unwrap().is_empty());
        assert_eq!(
            store
                .get_channel(&ImChannelId("im_1".into()))
                .unwrap()
                .cursor,
            None
        );

        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, before).unwrap();
        drop(store);
        let reopened = ImFileStore::open(dir.path()).unwrap();
        assert!(reopened.list_inbox().unwrap().is_empty());
        assert_eq!(
            reopened
                .get_channel(&ImChannelId("im_1".into()))
                .unwrap()
                .cursor,
            None
        );
    }

    #[test]
    fn durable_inbox_survives_restart_through_queue_and_ack() {
        let (dir, store) = store();
        store.insert_channel(record("im_1", "tg")).unwrap();
        let message = inbound("im_1", "7");
        let id = store
            .persist_inbound(message.clone(), Some("8".into()))
            .unwrap();
        drop(store);

        let store = ImFileStore::open(dir.path()).unwrap();
        let entries = store.list_inbox().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert_eq!(entries[0].message, message);
        assert!(!entries[0].queued);
        assert_eq!(
            store.get_channel(&message.channel_id).unwrap().cursor,
            None,
            "persisting an inbox entry must not advance the platform cursor"
        );
        store.queue_inbound(&id, Some("8".into())).unwrap();
        drop(store);

        let store = ImFileStore::open(dir.path()).unwrap();
        let entries = store.list_inbox().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].queued);
        assert_eq!(
            store
                .get_channel(&message.channel_id)
                .unwrap()
                .cursor
                .as_deref(),
            Some("8"),
            "queue and cursor must commit together before downstream ack"
        );
        store.ack_inbound(&id).unwrap();
        drop(store);

        let reopened = ImFileStore::open(dir.path()).unwrap();
        assert!(reopened.list_inbox().unwrap().is_empty());
        assert_eq!(
            reopened
                .get_channel(&message.channel_id)
                .unwrap()
                .cursor
                .as_deref(),
            Some("8")
        );
    }

    #[test]
    fn discard_removes_inbox_and_advances_cursor_in_one_rewrite() {
        let (_dir, store) = store();
        store.insert_channel(record("im_1", "tg")).unwrap();
        let id = store
            .persist_inbound(inbound("im_1", "7"), Some("8".into()))
            .unwrap();

        store.discard_inbound(&id, Some("8".into())).unwrap();

        assert!(store.list_inbox().unwrap().is_empty());
        assert_eq!(
            store
                .get_channel(&ImChannelId("im_1".into()))
                .unwrap()
                .cursor
                .as_deref(),
            Some("8")
        );
    }

    #[test]
    fn deleting_a_channel_preserves_orphaned_inbox_entries() {
        let (dir, store) = store();
        store.insert_channel(record("im_1", "tg")).unwrap();
        let id = store
            .persist_inbound(inbound("im_1", "7"), Some("8".into()))
            .unwrap();

        store.delete_channel(&ImChannelId("im_1".into())).unwrap();

        assert_eq!(store.list_inbox().unwrap()[0].id, id);
        let reopened = ImFileStore::open(dir.path()).unwrap();
        assert_eq!(reopened.list_inbox().unwrap()[0].id, id);
    }
}
