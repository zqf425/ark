//! Render-ready thread items (design §7.10.3).
//!
//! The mapping from raw events to display items happens **once**, on the Rust
//! side. The renderer only groups and orders these structures; it never
//! re-derives semantics from the 46 raw event kinds.

use ark_common::{ItemId, TurnId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
}

/// Text prefix rendered by `ark-agent`'s checkpoint family. Kept only as the
/// legacy fallback for items persisted before `marker` existed; new code must
/// match on [`ThreadItem::marker`] instead.
pub const CONTEXT_CHECKPOINT_PREFIX: &str = "[context checkpoint";

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
        }
    }

    pub fn with_marker(mut self, marker: ItemMarker) -> Self {
        self.marker = Some(marker);
        self
    }

    /// Whether this item carries a rendered context checkpoint.
    ///
    /// Matches the structural marker first; items persisted before the marker
    /// existed are recognized by their text prefix, which is safe here because
    /// the fallback only widens to *older* logs, never to user input.
    pub fn is_context_checkpoint(&self) -> bool {
        self.marker == Some(ItemMarker::ContextCheckpoint)
            || (self.marker.is_none()
                && self
                    .text
                    .as_deref()
                    .is_some_and(|text| text.starts_with(CONTEXT_CHECKPOINT_PREFIX)))
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
