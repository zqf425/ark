//! Render-ready thread items (design §7.10.3).
//!
//! The mapping from raw events to display items happens **once**, on the Rust
//! side. The renderer only groups and orders these structures; it never
//! re-derives semantics from the 46 raw event kinds.

use ark_common::{ItemId, TurnId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What a single item in the conversation stream represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Lifecycle of an item. Streaming text keeps `Streaming` until the turn or the
/// item terminates, so the renderer can show a caret without extra inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Pending,
    Streaming,
    Completed,
    Failed,
    Aborted,
}

/// Tool-call facets carried by `ToolCall` / `ToolResult` items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub duration_ms: Option<u64>,
}

/// One render-ready row of the conversation.
///
/// `text` holds the accumulated content: a full message for terminal items, and
/// the partially accumulated text for streaming ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        }
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
