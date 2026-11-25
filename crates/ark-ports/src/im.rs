//! IM channel persistence boundary (docs/requirements/im-channels.md).
//!
//! One small document (`<data_dir>/im.json`) holds every channel (with its
//! credentials and inbound cursor) and every chat→thread binding, so the port
//! is a plain document CRUD surface: channels change when the user edits
//! settings, bindings change once per new conversation.
//!
//! Sync like [`crate::ScheduleStore`], not `async` like
//! [`crate::SessionStore`]: the whole document is read or rewritten under one
//! in-process lock, so there is nothing to await and no benefit in handing the
//! critical section to the executor.
//!
//! ## Where policy lives
//!
//! The store assigns a channel id when the caller leaves one empty, persists
//! exactly what it is given, and never normalizes. Credential validation, the
//! channel cap and merge semantics live in `ark-im` / the server orchestration,
//! because a store that silently rewrote a channel would make the stored
//! document differ from the response the client just received.

use ark_common::Result;
use ark_contract::{ImBinding, ImChannel, ImChannelId};
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Stored per-channel record: the channel plus the service-maintained inbound
/// cursor.
///
/// Port-level type (same reasoning as [`crate::ThreadMeta`]: this trait must be
/// able to name its own persistence shape). Deliberately *not* exported to
/// TypeScript — the cursor is server bookkeeping, and no client can read or
/// write it. The cursor is a platform string (a Telegram update offset stored
/// as a decimal string, or a platform message id) because each adapter
/// defines what "already processed" means for its own protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImChannelRecord {
    pub channel: ImChannel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Platform-normalized inbound message persisted by the durable IM inbox.
///
/// The adapters report platform facts only. `channel_id` is stamped by the
/// delivery boundary before persistence; policy decisions remain in server
/// orchestration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundMessage {
    pub channel_id: ImChannelId,
    pub chat_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub message_id: String,
    pub text: String,
    pub is_dm: bool,
    pub is_mention: bool,
}

/// Server-generated durable inbox id. It is not a platform message id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImInboxId(String);

impl ImInboxId {
    pub fn new() -> Self {
        Self(format!("in_{}", uuid::Uuid::new_v4()))
    }
}

impl Default for ImInboxId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ImInboxId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One normalized message retained until downstream processing finishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImInboxEntry {
    pub id: ImInboxId,
    pub message: InboundMessage,
    /// Cursor that becomes safe only after every message in the platform batch
    /// has been persisted. Queueing this entry and advancing it are one write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// `false` means persisted but not yet admitted to a conversation queue.
    #[serde(default)]
    pub queued: bool,
    /// Store-assigned persistence order for this batch. It prevents a delayed
    /// older admission from moving the channel cursor backwards; absent in legacy files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_version: Option<u64>,
}

/// Durable channel table, chat→thread bindings and inbound inbox, owned by
/// `ark-store` and injected by the composition root.
pub trait ImStore: Send + Sync {
    /// Every stored channel record, most recently updated first.
    fn list_channels(&self) -> Result<Vec<ImChannelRecord>>;

    /// One channel record by id. Missing ids are `not_found`.
    fn get_channel(&self, id: &ImChannelId) -> Result<ImChannelRecord>;

    /// Append a channel, assigning the id when the caller left it empty.
    ///
    /// Returns the stored record, which is how the caller learns the generated
    /// id. A duplicate id is a `conflict` rather than an overwrite.
    fn insert_channel(&self, record: ImChannelRecord) -> Result<ImChannelRecord>;

    /// Overwrite an existing channel record (channel fields and cursor are one
    /// mutation, so they must not be two calls that could interleave).
    /// An absent id is `not_found`.
    fn replace_channel(&self, record: ImChannelRecord) -> Result<ImChannelRecord>;

    /// Remove one channel and — in the same locked critical section — every
    /// binding that points at it, so a deleted channel cannot leave orphaned
    /// chat→thread mappings behind. An absent id is `not_found`.
    fn delete_channel(&self, id: &ImChannelId) -> Result<()>;

    /// Bindings, optionally filtered by channel; most recently updated first.
    fn list_bindings(&self, channel_id: Option<&ImChannelId>) -> Result<Vec<ImBinding>>;

    /// The binding for one platform chat, if any. Absence is the normal
    /// first-message state, not an error.
    fn get_binding(&self, channel_id: &ImChannelId, chat_id: &str) -> Result<Option<ImBinding>>;

    /// Insert or overwrite the binding for `(channel_id, chat_id)`.
    fn set_binding(&self, binding: ImBinding) -> Result<()>;

    /// Remove one binding. Removing an absent binding is not an error: the
    /// `/new` command and the thread-missing cleanup race the same way.
    fn delete_binding(&self, channel_id: &ImChannelId, chat_id: &str) -> Result<()>;

    /// Durably append one normalized message before it enters the in-process
    /// delivery channel. The returned id identifies this specific delivery.
    fn persist_inbound(
        &self,
        message: InboundMessage,
        next_cursor: Option<String>,
    ) -> Result<ImInboxId>;

    /// Durably append a whole platform batch in one atomic document rewrite.
    ///
    /// Returns one id per input message in the same order. If any message is
    /// invalid or the rewrite fails, none of the batch may become visible and
    /// the channel cursor must remain unchanged.
    fn persist_inbound_batch(
        &self,
        messages: Vec<InboundMessage>,
        next_cursor: Option<String>,
    ) -> Result<Vec<ImInboxId>>;

    /// Every unacknowledged inbox entry in insertion order. Both pending and
    /// queued entries are replayed after a restart.
    fn list_inbox(&self) -> Result<Vec<ImInboxEntry>>;

    /// Advance past a platform batch containing no normalized messages. Callers
    /// must persist every normalized message in the batch before invoking it.
    fn advance_channel_cursor(&self, channel_id: &ImChannelId, next: Option<String>) -> Result<()>;

    /// Mark an entry admitted to a conversation queue and advance the channel
    /// cursor atomically. The entry stays durable until downstream processing
    /// finishes, closing the crash window between queue admission and ack.
    fn queue_inbound(&self, id: &ImInboxId, next_cursor: Option<String>) -> Result<()>;

    /// Delete an entry after downstream processing has consumed it.
    fn ack_inbound(&self, id: &ImInboxId) -> Result<()>;

    /// Atomically discard a terminally filtered/command entry and advance the
    /// cursor. Used only when no conversation work remains for the message.
    fn discard_inbound(&self, id: &ImInboxId, next_cursor: Option<String>) -> Result<()>;
}

/// A binding stamped with "now", for the call sites that do not care about the
/// exact instant.
pub fn binding_now(
    channel_id: ImChannelId,
    chat_id: impl Into<String>,
    thread_id: ark_common::ThreadId,
) -> ImBinding {
    ImBinding {
        channel_id,
        chat_id: chat_id.into(),
        thread_id,
        updated_at: Utc::now(),
    }
}
