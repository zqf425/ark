//! Event -> `ThreadItem` projection (design 7.10.3, 7.4.1 `items.jsonl`).
//!
//! This is the *only* place that turns raw events into render rows. Both the
//! incremental writer path (`items.jsonl`) and the rebuild path
//! (`project_items(events)`) run this same machine, so "rebuild equals
//! incremental" is true by construction and is asserted in the tests.
//!
//! `apply` is a thin dispatcher: each event family delegates to one small
//! handler, which keeps every function well under the review limit and makes
//! the fold order obvious.

use ark_common::{ItemId, TurnId};
use ark_contract::item::{ItemKind, ToolPayload};
use ark_contract::{ItemStatus, RuntimeEvent, ThreadItem};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

/// Stable synthetic `ItemId` for events that only carry a string key
/// (`call_id`, `approval_id`, `request_id`, `seq`).
///
/// Deterministic on purpose: a rebuild must produce the same ids as the
/// incremental log, otherwise the renderer would remount every row.
pub fn keyed_item_id(namespace: &str) -> ItemId {
    let hi = fnv1a(namespace.as_bytes(), 0xcbf2_9ce4_8422_2325);
    let lo = fnv1a(namespace.as_bytes(), 0x8422_2325_cbf2_9ce4);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&hi.to_be_bytes());
    bytes[8..].copy_from_slice(&lo.to_be_bytes());
    // Force a valid variant and a non-standard version nibble (8 = custom).
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    ItemId::from_uuid(Uuid::from_bytes(bytes))
}

fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The zero turn id used when an event has no turn (diagnostics only).
fn orphan_turn() -> TurnId {
    TurnId::from_uuid(Uuid::nil())
}

fn build(id: ItemId, turn: TurnId, kind: ItemKind, at: DateTime<Utc>) -> ThreadItem {
    let mut item = ThreadItem::new(turn, kind);
    item.item_id = id;
    item.created_at = at;
    item
}

/// Incremental folder: keeps item order and the latest snapshot per item.
#[derive(Debug, Default)]
pub struct ItemProjector {
    order: Vec<ItemId>,
    items: HashMap<ItemId, ThreadItem>,
    current_turn: Option<TurnId>,
}

impl ItemProjector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one event; returns the updated snapshot of the touched item.
    pub fn apply(&mut self, event: &RuntimeEvent) -> Option<ThreadItem> {
        self.note_turn(event);
        let item = match event {
            RuntimeEvent::ItemCreated { item, .. } | RuntimeEvent::ItemUpdated { item, .. } => {
                self.upsert(item.clone())
            }
            RuntimeEvent::ItemCompleted { item, .. } => self.apply_item_completed(item),
            RuntimeEvent::AssistantTextDelta {
                turn_id,
                item_id,
                timestamp,
                delta,
                ..
            } => self.apply_delta(
                *item_id,
                *turn_id,
                *timestamp,
                ItemKind::AssistantMessage,
                delta,
            ),
            RuntimeEvent::AssistantReasoningDelta {
                turn_id,
                item_id,
                timestamp,
                delta,
                ..
            } => self.apply_delta(*item_id, *turn_id, *timestamp, ItemKind::Reasoning, delta),
            RuntimeEvent::ToolCallStarted {
                turn_id,
                call_id,
                tool,
                arguments,
                timestamp,
                ..
            } => self.apply_tool_started(*turn_id, call_id, tool, arguments, *timestamp),
            RuntimeEvent::ToolCallFinished {
                turn_id,
                call_id,
                tool,
                ok,
                duration_ms,
                output,
                detail,
                timestamp,
                ..
            } => self.apply_tool_finished(
                *turn_id,
                call_id,
                tool,
                *ok,
                *duration_ms,
                output,
                detail.as_deref(),
                *timestamp,
            ),
            RuntimeEvent::ApprovalRequested {
                turn_id,
                approval_id,
                tool,
                summary,
                timestamp,
                ..
            } => self.apply_approval_requested(*turn_id, approval_id, tool, summary, *timestamp),
            RuntimeEvent::ApprovalResolved {
                turn_id,
                approval_id,
                approved,
                timestamp,
                ..
            } => self.apply_approval_resolved(*turn_id, approval_id, *approved, *timestamp),
            RuntimeEvent::UserInputRequested {
                turn_id,
                request_id,
                prompt,
                timestamp,
                ..
            } => self.apply_user_input_requested(*turn_id, request_id, prompt, *timestamp),
            RuntimeEvent::UserInputResolved {
                turn_id,
                request_id,
                answer,
                timestamp,
                ..
            } => self.apply_user_input_resolved(*turn_id, request_id, answer, *timestamp),
            RuntimeEvent::Error {
                seq,
                timestamp,
                message,
                ..
            } => self.apply_error(*seq, message, *timestamp),
            RuntimeEvent::CompactionCompleted {
                turn_id,
                timestamp,
                removed_items,
                ..
            } => self.apply_compaction(*turn_id, *removed_items, *timestamp),
            _ => return None,
        };
        Some(item)
    }

    /// Fold every event and return items in first-appearance order.
    pub fn finish(self) -> Vec<ThreadItem> {
        self.order
            .into_iter()
            .filter_map(|id| self.items.get(&id).cloned())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn apply_item_completed(&mut self, incoming: &ThreadItem) -> ThreadItem {
        let mut item = incoming.clone();
        if !matches!(item.status, ItemStatus::Failed | ItemStatus::Aborted) {
            item.status = ItemStatus::Completed;
        }
        self.upsert(item)
    }

    fn apply_delta(
        &mut self,
        id: ItemId,
        turn: TurnId,
        at: DateTime<Utc>,
        kind: ItemKind,
        delta: &str,
    ) -> ThreadItem {
        let mut item = self.ensure(id, turn, kind, at);
        item.push_delta(delta);
        self.upsert(item)
    }

    fn apply_tool_started(
        &mut self,
        turn: TurnId,
        call_id: &str,
        tool: &str,
        arguments: &serde_json::Value,
        at: DateTime<Utc>,
    ) -> ThreadItem {
        let id = keyed_item_id(&format!("tool_call:{call_id}"));
        let mut item = self.ensure(id, turn, ItemKind::ToolCall, at);
        let payload = tool_payload(&mut item, call_id, tool);
        payload.tool = tool.to_string();
        payload.arguments = (!arguments.is_null()).then(|| arguments.clone());
        item.status = ItemStatus::Streaming;
        self.upsert(item)
    }

    fn apply_tool_finished(
        &mut self,
        turn: TurnId,
        call_id: &str,
        tool: &str,
        ok: bool,
        duration_ms: u64,
        output: &str,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> ThreadItem {
        let id = keyed_item_id(&format!("tool_call:{call_id}"));
        let mut item = self.ensure(id, turn, ItemKind::ToolCall, at);
        let payload = tool_payload(&mut item, call_id, tool);
        payload.ok = Some(ok);
        payload.duration_ms = Some(duration_ms);
        payload.output = (!output.is_empty()).then(|| output.to_string());
        item.detail = detail.map(ToString::to_string);
        item.status = if ok {
            ItemStatus::Completed
        } else {
            ItemStatus::Failed
        };
        self.upsert(item)
    }

    fn apply_approval_requested(
        &mut self,
        turn: TurnId,
        approval_id: &str,
        tool: &str,
        summary: &str,
        at: DateTime<Utc>,
    ) -> ThreadItem {
        let id = keyed_item_id(&format!("approval:{approval_id}"));
        let mut item = build(id, turn, ItemKind::Approval, at);
        item.text = Some(summary.to_string());
        item.detail = Some(tool.to_string());
        self.upsert(item)
    }

    fn apply_approval_resolved(
        &mut self,
        turn: TurnId,
        approval_id: &str,
        approved: bool,
        at: DateTime<Utc>,
    ) -> ThreadItem {
        let id = keyed_item_id(&format!("approval:{approval_id}"));
        let mut item = self.ensure(id, turn, ItemKind::Approval, at);
        item.status = ItemStatus::Completed;
        item.detail = Some(if approved {
            "approved".to_string()
        } else {
            "rejected".to_string()
        });
        self.upsert(item)
    }

    fn apply_user_input_requested(
        &mut self,
        turn: TurnId,
        request_id: &str,
        prompt: &str,
        at: DateTime<Utc>,
    ) -> ThreadItem {
        let id = keyed_item_id(&format!("user_input:{request_id}"));
        let mut item = build(id, turn, ItemKind::UserInput, at);
        item.text = Some(prompt.to_string());
        self.upsert(item)
    }

    fn apply_user_input_resolved(
        &mut self,
        turn: TurnId,
        request_id: &str,
        answer: &str,
        at: DateTime<Utc>,
    ) -> ThreadItem {
        let id = keyed_item_id(&format!("user_input:{request_id}"));
        let mut item = self.ensure(id, turn, ItemKind::UserInput, at);
        item.status = ItemStatus::Completed;
        item.text = Some(answer.to_string());
        self.upsert(item)
    }

    fn apply_error(&mut self, seq: u64, message: &str, at: DateTime<Utc>) -> ThreadItem {
        let id = keyed_item_id(&format!("error:{seq}"));
        let turn = self.current_turn.unwrap_or_else(orphan_turn);
        let mut item = build(id, turn, ItemKind::Error, at);
        item.text = Some(message.to_string());
        item.status = ItemStatus::Failed;
        self.upsert(item)
    }

    fn apply_compaction(&mut self, turn: TurnId, removed: u32, at: DateTime<Utc>) -> ThreadItem {
        let id = keyed_item_id(&format!("compaction:{turn}"));
        let mut item = build(id, turn, ItemKind::Notice, at);
        item.text = Some(format!("{removed} earlier items compacted"));
        item.status = ItemStatus::Completed;
        self.upsert(item)
    }

    fn note_turn(&mut self, event: &RuntimeEvent) {
        let turn = match event {
            RuntimeEvent::TurnStarted { turn_id, .. }
            | RuntimeEvent::TurnCompleted { turn_id, .. }
            | RuntimeEvent::TurnFailed { turn_id, .. }
            | RuntimeEvent::TurnAborted { turn_id, .. }
            | RuntimeEvent::TurnSteered { turn_id, .. }
            | RuntimeEvent::AssistantTextDelta { turn_id, .. }
            | RuntimeEvent::AssistantReasoningDelta { turn_id, .. }
            | RuntimeEvent::ItemCreated { turn_id, .. }
            | RuntimeEvent::ItemUpdated { turn_id, .. }
            | RuntimeEvent::ItemCompleted { turn_id, .. }
            | RuntimeEvent::ToolCallStarted { turn_id, .. }
            | RuntimeEvent::ToolCallFinished { turn_id, .. }
            | RuntimeEvent::ApprovalRequested { turn_id, .. }
            | RuntimeEvent::ApprovalResolved { turn_id, .. }
            | RuntimeEvent::UserInputRequested { turn_id, .. }
            | RuntimeEvent::UserInputResolved { turn_id, .. }
            | RuntimeEvent::Usage { turn_id, .. } => Some(*turn_id),
            _ => None,
        };
        if let Some(turn) = turn {
            self.current_turn = Some(turn);
        }
    }

    fn ensure(
        &mut self,
        id: ItemId,
        turn: TurnId,
        kind: ItemKind,
        at: DateTime<Utc>,
    ) -> ThreadItem {
        if let Some(existing) = self.items.get(&id) {
            return existing.clone();
        }
        let item = build(id, turn, kind, at);
        self.order.push(id);
        self.items.insert(id, item.clone());
        item
    }

    fn upsert(&mut self, item: ThreadItem) -> ThreadItem {
        if !self.items.contains_key(&item.item_id) {
            self.order.push(item.item_id);
        }
        self.items.insert(item.item_id, item.clone());
        item
    }
}

fn tool_payload<'a>(item: &'a mut ThreadItem, call_id: &str, tool: &str) -> &'a mut ToolPayload {
    item.tool.get_or_insert_with(|| ToolPayload {
        call_id: call_id.to_string(),
        tool: tool.to_string(),
        arguments: None,
        output: None,
        ok: None,
        duration_ms: None,
    })
}

/// Fold snapshots (as stored in `items.jsonl`) back into the ordered stream.
///
/// First appearance decides the order, the last snapshot wins: identical to
/// replaying the events that produced the snapshots.
pub fn fold_item_snapshots(snapshots: impl IntoIterator<Item = ThreadItem>) -> Vec<ThreadItem> {
    let mut order: Vec<ItemId> = Vec::new();
    let mut items: HashMap<ItemId, ThreadItem> = HashMap::new();
    for item in snapshots {
        if !items.contains_key(&item.item_id) {
            order.push(item.item_id);
        }
        items.insert(item.item_id, item);
    }
    order
        .into_iter()
        .filter_map(|id| items.remove(&id))
        .collect()
}

/// Pure rebuild: the reference implementation for `items.jsonl`.
pub fn project_items(events: &[RuntimeEvent]) -> Vec<ThreadItem> {
    let mut projector = ItemProjector::new();
    for event in events {
        projector.apply(event);
    }
    projector.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn() -> TurnId {
        TurnId::new()
    }

    #[test]
    fn keyed_ids_are_stable_and_distinct() {
        let a = keyed_item_id("tool_call:c1");
        let b = keyed_item_id("tool_call:c1");
        let c = keyed_item_id("tool_call:c2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.to_string().len(), 36);
    }

    #[test]
    fn text_deltas_accumulate_into_one_item() {
        let turn = turn();
        let item = ItemId::new();
        let now = Utc::now();
        let thread = ark_common::ThreadId::new();
        let events = vec![
            RuntimeEvent::AssistantTextDelta {
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                item_id: item,
                timestamp: now,
                delta: "he".into(),
            },
            RuntimeEvent::AssistantTextDelta {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                item_id: item,
                timestamp: now,
                delta: "llo".into(),
            },
        ];
        let items = project_items(&events);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text.as_deref(), Some("hello"));
        assert_eq!(items[0].status, ItemStatus::Streaming);
        assert_eq!(items[0].kind, ItemKind::AssistantMessage);
    }

    #[test]
    fn tool_lifecycle_folds_into_one_completed_item() {
        let turn = turn();
        let thread = ark_common::ThreadId::new();
        let now = Utc::now();
        let events = vec![
            RuntimeEvent::ToolCallStarted {
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: now,
                call_id: "call_1".into(),
                tool: "read".into(),
                arguments: serde_json::json!({ "path": "src/main.rs" }),
            },
            RuntimeEvent::ToolCallFinished {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                timestamp: now,
                call_id: "call_1".into(),
                tool: "read".into(),
                ok: true,
                duration_ms: 12,
                output: "fn main() {}".into(),
                detail: None,
            },
        ];
        let items = project_items(&events);
        assert_eq!(items.len(), 1);
        let tool = items[0].tool.as_ref().expect("tool payload");
        assert_eq!(tool.tool, "read");
        assert_eq!(
            tool.arguments,
            Some(serde_json::json!({ "path": "src/main.rs" }))
        );
        assert_eq!(tool.output.as_deref(), Some("fn main() {}"));
        assert_eq!(tool.ok, Some(true));
        assert_eq!(tool.duration_ms, Some(12));
        assert_eq!(items[0].status, ItemStatus::Completed);
    }

    #[test]
    fn approvals_and_user_input_keep_their_prompt_text() {
        let turn = turn();
        let thread = ark_common::ThreadId::new();
        let now = Utc::now();
        let events = vec![
            RuntimeEvent::ApprovalRequested {
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: now,
                approval_id: "ap_1".into(),
                tool: "bash".into(),
                summary: "run rm".into(),
            },
            RuntimeEvent::UserInputRequested {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                timestamp: now,
                request_id: "ui_1".into(),
                prompt: "which file?".into(),
            },
        ];
        let items = project_items(&events);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, ItemKind::Approval);
        assert_eq!(items[0].text.as_deref(), Some("run rm"));
        assert_eq!(items[1].kind, ItemKind::UserInput);
        assert_eq!(items[1].text.as_deref(), Some("which file?"));
    }

    #[test]
    fn errors_are_attributed_to_the_current_turn() {
        let turn = turn();
        let thread = ark_common::ThreadId::new();
        let now = Utc::now();
        let events = vec![
            RuntimeEvent::TurnStarted {
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: now,
            },
            RuntimeEvent::Error {
                seq: 2,
                thread_id: thread,
                timestamp: now,
                code: ark_common::ArkErrorCode::ModelRequestFailed,
                message: "502".into(),
            },
        ];
        let items = project_items(&events);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].turn_id, turn);
        assert_eq!(items[0].status, ItemStatus::Failed);
    }

    #[test]
    fn fold_of_snapshots_matches_the_projector_output() {
        let turn = turn();
        let thread = ark_common::ThreadId::new();
        let now = Utc::now();
        let item = ItemId::new();
        let events = vec![
            RuntimeEvent::AssistantTextDelta {
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                item_id: item,
                timestamp: now,
                delta: "a".into(),
            },
            RuntimeEvent::AssistantTextDelta {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                item_id: item,
                timestamp: now,
                delta: "b".into(),
            },
        ];
        let mut projector = ItemProjector::new();
        let mut snapshots = Vec::new();
        for event in &events {
            if let Some(snapshot) = projector.apply(event) {
                snapshots.push(snapshot);
            }
        }
        assert_eq!(fold_item_snapshots(snapshots), project_items(&events));
    }

    #[test]
    fn unrelated_events_project_to_nothing() {
        let thread = ark_common::ThreadId::new();
        let events = vec![RuntimeEvent::Heartbeat {
            seq: 1,
            thread_id: thread,
            timestamp: Utc::now(),
        }];
        assert!(project_items(&events).is_empty());
    }
}
