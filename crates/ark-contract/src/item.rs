//! Render-ready thread items (design §7.10.3).
//!
//! The mapping from raw events to display items happens **once**, on the Rust
//! side. The renderer only groups and orders these structures; it never
//! re-derives semantics from the 46 raw event kinds.

use ark_common::{ItemId, TurnId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::event::TurnAttachmentOutcome;

/// What a single item in the conversation stream represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    UserMessage,
    AssistantMessage,
    Reasoning,
    ToolCall,
    ToolResult,
    Approval,
    UserInput,
    Error,
    Notice,
}

/// Structural marker for system-generated item families that a renderer or a
/// projection must recognize without parsing their text. Text prefixes are not
/// identity: a user can type any prefix, so matching on it would let user input
/// masquerade as generated state (or hide real checkpoints after a render-format
/// change).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ItemMarker {
    /// Rendered context checkpoint produced by compaction (`ark-agent`).
    ContextCheckpoint,
}

/// Lifecycle of an item. Streaming text keeps `Streaming` until the turn or the
/// item terminates, so the renderer can show a caret without extra inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Pending,
    Streaming,
    Completed,
    Failed,
    Aborted,
}

/// Tool-call facets carried by `ToolCall` / `ToolResult` items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ToolPayload {
    pub call_id: String,
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub duration_ms: Option<u64>,
}

/// One render-ready row of the conversation.
///
/// `text` holds the accumulated content: a full message for terminal items, and
/// the partially accumulated text for streaming ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ThreadItem {
    pub item_id: ItemId,
    pub turn_id: TurnId,
    pub kind: ItemKind,
    pub status: ItemStatus,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<ItemMarker>,
    /// Per-attachment delivery record for a `UserMessage` that seeded the turn
    /// with attachments. Rides the item (not only the `turn_started` event) so
    /// history replay and thread detail render the same chips as the live
    /// stream; `#[serde(default)]` keeps pre-existing items readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<TurnAttachmentOutcome>>,
}

impl ThreadItem {
    pub fn new(turn_id: TurnId, kind: ItemKind) -> Self {
        Self {
            item_id: ItemId::new(),
            turn_id,
            kind,
            status: ItemStatus::Pending,
            created_at: Utc::now(),
            text: None,
            tool: None,
            detail: None,
            marker: None,
            attachments: None,
        }
    }

    pub fn with_marker(mut self, marker: ItemMarker) -> Self {
        self.marker = Some(marker);
        self
    }

    pub fn with_attachments(mut self, attachments: Vec<TurnAttachmentOutcome>) -> Self {
        self.attachments = Some(attachments);
        self
    }

    /// Whether this item carries a rendered context checkpoint.
    ///
    /// Marker-only on purpose: the project has no stored history predating the
    /// marker, so a text-prefix fallback would buy nothing and reopen the hole
    /// the marker closed (a user message can open with the same text).
    pub fn is_context_checkpoint(&self) -> bool {
        self.kind == ItemKind::Notice && self.marker == Some(ItemMarker::ContextCheckpoint)
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn completed(mut self) -> Self {
        self.status = ItemStatus::Completed;
        self
    }

    /// Append a streaming delta, flipping the item into `Streaming`.
    pub fn push_delta(&mut self, delta: &str) {
        let buf = self.text.get_or_insert_with(String::new);
        buf.push_str(delta);
        if self.status == ItemStatus::Pending {
            self.status = ItemStatus::Streaming;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_accumulate_and_flip_status() {
        let mut item = ThreadItem::new(TurnId::new(), ItemKind::AssistantMessage);
        assert_eq!(item.status, ItemStatus::Pending);
        item.push_delta("he");
        item.push_delta("llo");
        assert_eq!(item.text.as_deref(), Some("hello"));
        assert_eq!(item.status, ItemStatus::Streaming);
    }

    #[test]
    fn completed_item_roundtrips_as_snake_case() {
        let item = ThreadItem::new(TurnId::new(), ItemKind::ToolCall).completed();
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["kind"], "tool_call");
        assert_eq!(json["status"], "completed");
    }
}

#[cfg(test)]
mod marker_tests {
    use super::*;

    #[test]
    fn a_marked_notice_is_a_checkpoint() {
        let item = ThreadItem::new(TurnId::new(), ItemKind::Notice)
            .with_text("[context checkpoint]\nGoal: x")
            .with_marker(ItemMarker::ContextCheckpoint);
        assert!(item.is_context_checkpoint());
    }

    #[test]
    fn a_user_message_is_never_a_checkpoint_even_with_the_prefix() {
        // The regression this marker exists for: the prefix is text a user can
        // type, so the fallback must not escape the Notice kind.
        let item = ThreadItem::new(TurnId::new(), ItemKind::UserMessage)
            .with_text("[context checkpoint] 我手打的")
            .completed();
        assert!(!item.is_context_checkpoint());
    }

    #[test]
    fn a_notice_without_marker_or_prefix_is_not_a_checkpoint() {
        let item = ThreadItem::new(TurnId::new(), ItemKind::Notice).with_text("普通通知");
        assert!(!item.is_context_checkpoint());
    }
}
