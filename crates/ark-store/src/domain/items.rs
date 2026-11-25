//! Shared event-to-item projection for incremental writes and full rebuilds.
//! Small per-family handlers keep rebuild and streaming folds identical.

pub use ark_common::keyed_item_id;
use ark_common::{ItemId, TurnId};
use ark_contract::item::{ItemKind, ToolPayload};
use ark_contract::{ItemStatus, RuntimeEvent, ThreadItem};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

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

/// The `ToolCallFinished` fields the projection reads, grouped as one value.
struct ToolFinished<'a> {
    call_id: &'a str,
    tool: &'a str,
    ok: bool,
    duration_ms: u64,
    output: &'a str,
    detail: Option<&'a str>,
}

/// One snapshot plus its covered event interval; deltas append no rows because
/// cumulative text would make `items.jsonl` grow quadratically with replies.
/// `first_seq` preserves render order; `watermark_seq` prevents delta replay
/// from duplicating text. Legacy rows with both fields at zero require a full rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemSnapshotRecord {
    #[serde(default)]
    pub first_seq: u64,
    #[serde(default)]
    pub watermark_seq: u64,
    #[serde(flatten)]
    pub item: ThreadItem,
}

impl ItemSnapshotRecord {
    /// Event seqs start at 1, so zero marks legacy text that already includes every delta.
    pub fn is_legacy_shape(&self) -> bool {
        self.watermark_seq == 0
    }

    /// Wrap a freshly projected item with the positions this projection
    /// already covers, so a later read can replay only what is missing.
    pub fn from_projection(first_seq: u64, watermark_seq: u64, item: &ThreadItem) -> Self {
        Self {
            first_seq,
            watermark_seq,
            item: item.clone(),
        }
    }
}

/// Whether this event is a streaming delta append.
///
/// Only the two assistant delta kinds carry one text fragment per event; every
/// other kind is a key point the derived files may persist directly. Keep this
/// in sync with the projector's delta handlers.
pub fn is_delta_event(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::AssistantTextDelta { .. } | RuntimeEvent::AssistantReasoningDelta { .. }
    )
}

/// The item a delta event appends to.
pub fn delta_item_id(event: &RuntimeEvent) -> Option<ItemId> {
    match event {
        RuntimeEvent::AssistantTextDelta { item_id, .. }
        | RuntimeEvent::AssistantReasoningDelta { item_id, .. } => Some(*item_id),
        _ => None,
    }
}

/// Stable target item for every event handled by the item projector.
pub(crate) fn projected_item_id(event: &RuntimeEvent) -> Option<ItemId> {
    match event {
        RuntimeEvent::ItemCreated { item, .. }
        | RuntimeEvent::ItemUpdated { item, .. }
        | RuntimeEvent::ItemCompleted { item, .. } => Some(item.item_id),
        RuntimeEvent::AssistantTextDelta { item_id, .. }
        | RuntimeEvent::AssistantReasoningDelta { item_id, .. } => Some(*item_id),
        RuntimeEvent::ToolCallStarted {
            item_id, call_id, ..
        }
        | RuntimeEvent::ToolCallFinished {
            item_id, call_id, ..
        } => Some((*item_id).unwrap_or_else(|| keyed_item_id(&format!("tool_call:{call_id}")))),
        RuntimeEvent::ApprovalRequested { approval_id, .. }
        | RuntimeEvent::ApprovalResolved { approval_id, .. } => {
            Some(keyed_item_id(&format!("approval:{approval_id}")))
        }
        RuntimeEvent::UserInputRequested {
            item_id,
            request_id,
            ..
        }
        | RuntimeEvent::UserInputResolved {
            item_id,
            request_id,
            ..
        } => Some((*item_id).unwrap_or_else(|| keyed_item_id(&format!("user_input:{request_id}")))),
        RuntimeEvent::Error { seq, .. } => Some(keyed_item_id(&format!("error:{seq}"))),
        // A terminal `turn_failed` is the interruption point the renderer must
        // show inline: the projector folds it into one durable `error` item
        // attributed to the failed turn, so the live stream and a replay after
        // a restart render the same row at the same place.
        RuntimeEvent::TurnFailed {
            item_id, turn_id, ..
        } => Some((*item_id).unwrap_or_else(|| keyed_item_id(&format!("turn_failed:{turn_id}")))),
        // `CompactionCompleted` is a control event, not a render row. The
        // reactor already appended one `ItemCreated` checkpoint notice for the
        // same compaction; projecting it again produced a second, redundant
        // "N earlier items compacted" card. Keep the event in the log for
        // diagnostics/usage, but never fold it into `items.jsonl`.
        _ => None,
    }
}

pub(crate) fn backfill_legacy_item_id(event: &mut RuntimeEvent) {
    let fallback = projected_item_id(event);
    match event {
        RuntimeEvent::ToolCallStarted { item_id, .. }
        | RuntimeEvent::ToolCallFinished { item_id, .. }
        | RuntimeEvent::UserInputRequested { item_id, .. }
        | RuntimeEvent::UserInputResolved { item_id, .. }
        | RuntimeEvent::TurnFailed { item_id, .. }
            if item_id.is_none() =>
        {
            *item_id = fallback;
        }
        _ => {}
    }
}

/// Whether appending this event can change any persisted item snapshot.
pub(crate) fn affects_items(event: &RuntimeEvent) -> bool {
    projected_item_id(event).is_some()
}

/// Incremental folder: keeps item order and the latest snapshot per item.
#[derive(Debug, Default, Clone)]
pub struct ItemProjector {
    order: Vec<ItemId>,
    items: HashMap<ItemId, ThreadItem>,
    /// First event `seq` that touched each item (render order on rebuild).
    first: HashMap<ItemId, u64>,
    /// Last event `seq` folded into each item (snapshot watermark).
    touched: HashMap<ItemId, u64>,
    current_turn: Option<TurnId>,
}

impl ItemProjector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed the projector with the newest row read back from `items.jsonl`,
    /// together with the event interval that row already covers.
    ///
    /// Replaying only events past the row's watermark then continues the
    /// projection exactly where the snapshot stopped, so a read does not have
    /// to fold the whole log again.
    pub fn seed_snapshot(&mut self, first_seq: u64, watermark_seq: u64, item: ThreadItem) {
        let item_id = item.item_id;
        if !self.items.contains_key(&item_id) {
            self.order.push(item_id);
        }
        self.first.entry(item_id).or_insert(first_seq);
        self.items.insert(item_id, item);
        self.touched.insert(item_id, watermark_seq);
    }

    /// Restore turn context that may only exist in a compacted session checkpoint.
    pub(crate) fn seed_current_turn(&mut self, turn_id: Option<TurnId>) {
        if turn_id.is_some() {
            self.current_turn = turn_id;
        }
    }

    pub(crate) fn current_turn(&self) -> Option<TurnId> {
        self.current_turn
    }

    /// Apply one event; returns the updated snapshot of the touched item.
    pub fn apply(&mut self, event: &RuntimeEvent) -> Option<ThreadItem> {
        let id = self.apply_in_place(event)?;
        self.items.get(&id).cloned()
    }

    /// Apply without cloning; only callers that need a persisted snapshot clone it.
    pub(crate) fn apply_in_place(&mut self, event: &RuntimeEvent) -> Option<ItemId> {
        self.note_turn(event);
        self.apply_projected(event)
    }

    /// Skip covered items while still observing the turn used by later errors.
    pub(crate) fn apply_after_watermark(&mut self, event: &RuntimeEvent) -> Option<ItemId> {
        self.note_turn(event);
        if projected_item_id(event)
            .is_some_and(|item_id| self.watermark_of(&item_id) >= event.seq())
        {
            return None;
        }
        self.apply_projected(event)
    }

    fn apply_projected(&mut self, event: &RuntimeEvent) -> Option<ItemId> {
        let seq = event.seq();
        let item_id = match event {
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
                item_id,
                call_id,
                tool,
                arguments,
                timestamp,
                ..
            } => {
                let id =
                    (*item_id).unwrap_or_else(|| keyed_item_id(&format!("tool_call:{call_id}")));
                self.apply_tool_started(id, *turn_id, call_id, tool, arguments, *timestamp)
            }
            RuntimeEvent::ToolCallFinished {
                turn_id,
                item_id,
                call_id,
                tool,
                ok,
                duration_ms,
                output,
                detail,
                timestamp,
                ..
            } => {
                let id =
                    (*item_id).unwrap_or_else(|| keyed_item_id(&format!("tool_call:{call_id}")));
                self.apply_tool_finished(
                    id,
                    *turn_id,
                    &ToolFinished {
                        call_id,
                        tool,
                        ok: *ok,
                        duration_ms: *duration_ms,
                        output,
                        detail: detail.as_deref(),
                    },
                    *timestamp,
                )
            }
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
                item_id,
                request_id,
                prompt,
                timestamp,
                ..
            } => {
                let id = (*item_id)
                    .unwrap_or_else(|| keyed_item_id(&format!("user_input:{request_id}")));
                self.apply_user_input_requested(id, *turn_id, prompt, *timestamp)
            }
            RuntimeEvent::UserInputResolved {
                turn_id,
                item_id,
                request_id,
                answer,
                timestamp,
                ..
            } => {
                let id = (*item_id)
                    .unwrap_or_else(|| keyed_item_id(&format!("user_input:{request_id}")));
                self.apply_user_input_resolved(id, *turn_id, answer, *timestamp)
            }
            RuntimeEvent::Error {
                seq,
                timestamp,
                message,
                ..
            } => self.apply_error(*seq, message, *timestamp),
            RuntimeEvent::TurnFailed {
                turn_id,
                item_id,
                timestamp,
                message,
                ..
            } => {
                let id =
                    (*item_id).unwrap_or_else(|| keyed_item_id(&format!("turn_failed:{turn_id}")));
                self.apply_turn_failed(id, *turn_id, message, *timestamp)
            }
            _ => return None,
        };
        self.first.entry(item_id).or_insert(seq);
        self.touched.insert(item_id, seq);
        Some(item_id)
    }

    /// First event `seq` that touched the item (render order on rebuild).
    pub fn first_seq_of(&self, item_id: &ItemId) -> u64 {
        self.first.get(item_id).copied().unwrap_or(0)
    }

    /// Last event `seq` folded into the item (0 when it was never touched).
    pub fn watermark_of(&self, item_id: &ItemId) -> u64 {
        self.touched.get(item_id).copied().unwrap_or(0)
    }

    pub(crate) fn item(&self, item_id: &ItemId) -> Option<&ThreadItem> {
        self.items.get(item_id)
    }

    /// Fold every event and return items in first-appearance order.
    pub fn finish(self) -> Vec<ThreadItem> {
        self.order
            .into_iter()
            .filter_map(|id| self.items.get(&id).cloned())
            .collect()
    }

    /// The final snapshot row per item, in first-appearance order.
    ///
    /// Each row carries the event interval this projection already covered, so
    /// a watermark-aware reader can resume from the row instead of folding the
    /// whole log again (used when `items.jsonl` is rewritten wholesale).
    pub fn snapshot_records(self) -> Vec<ItemSnapshotRecord> {
        self.order
            .into_iter()
            .filter_map(|id| {
                let item = self.items.get(&id)?;
                Some(ItemSnapshotRecord::from_projection(
                    self.first.get(&id).copied().unwrap_or(0),
                    self.touched.get(&id).copied().unwrap_or(0),
                    item,
                ))
            })
            .collect()
    }

    /// Final items in event order, for a projection seeded from snapshot rows.
    ///
    /// Seeding happens in file order and the delta replay then touches items
    /// scattered across the log, so the incremental `order` vector no longer
    /// reflects the event stream. Sorting by the first touching event keeps the
    /// read-back equal to a full `project_items` replay; `first_seq == 0` can
    /// only come from a legacy row, which never reaches this path.
    pub fn finish_in_event_order(self) -> Vec<ThreadItem> {
        let mut entries: Vec<(u64, ThreadItem)> = self
            .items
            .into_iter()
            .map(|(id, item)| (self.first.get(&id).copied().unwrap_or(0), item))
            .collect();
        entries.sort_by_key(|(first, _)| *first);
        entries.into_iter().map(|(_, item)| item).collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn apply_item_completed(&mut self, incoming: &ThreadItem) -> ItemId {
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
    ) -> ItemId {
        let id = self.ensure(id, turn, kind, at);
        self.items
            .get_mut(&id)
            .expect("ensure inserted the delta item")
            .push_delta(delta);
        id
    }

    fn apply_tool_started(
        &mut self,
        id: ItemId,
        turn: TurnId,
        call_id: &str,
        tool: &str,
        arguments: &serde_json::Value,
        at: DateTime<Utc>,
    ) -> ItemId {
        let id = self.ensure(id, turn, ItemKind::ToolCall, at);
        let item = self
            .items
            .get_mut(&id)
            .expect("ensure inserted the tool item");
        let payload = tool_payload(item, call_id, tool);
        payload.tool = tool.to_string();
        payload.arguments = (!arguments.is_null()).then(|| arguments.clone());
        item.status = ItemStatus::Streaming;
        id
    }

    fn apply_tool_finished(
        &mut self,
        id: ItemId,
        turn: TurnId,
        finished: &ToolFinished<'_>,
        at: DateTime<Utc>,
    ) -> ItemId {
        let id = self.ensure(id, turn, ItemKind::ToolCall, at);
        let item = self
            .items
            .get_mut(&id)
            .expect("ensure inserted the tool item");
        let payload = tool_payload(item, finished.call_id, finished.tool);
        payload.ok = Some(finished.ok);
        payload.duration_ms = Some(finished.duration_ms);
        payload.output = (!finished.output.is_empty()).then(|| finished.output.to_string());
        item.detail = finished.detail.map(ToString::to_string);
        item.status = if finished.ok {
            ItemStatus::Completed
        } else {
            ItemStatus::Failed
        };
        id
    }

    fn apply_approval_requested(
        &mut self,
        turn: TurnId,
        approval_id: &str,
        tool: &str,
        summary: &str,
        at: DateTime<Utc>,
    ) -> ItemId {
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
    ) -> ItemId {
        let id = keyed_item_id(&format!("approval:{approval_id}"));
        let id = self.ensure(id, turn, ItemKind::Approval, at);
        let item = self
            .items
            .get_mut(&id)
            .expect("ensure inserted the approval item");
        item.status = ItemStatus::Completed;
        item.detail = Some(if approved {
            "approved".to_string()
        } else {
            "rejected".to_string()
        });
        id
    }

    fn apply_user_input_requested(
        &mut self,
        id: ItemId,
        turn: TurnId,
        prompt: &str,
        at: DateTime<Utc>,
    ) -> ItemId {
        let mut item = build(id, turn, ItemKind::UserInput, at);
        item.text = Some(prompt.to_string());
        self.upsert(item)
    }

    fn apply_user_input_resolved(
        &mut self,
        id: ItemId,
        turn: TurnId,
        answer: &str,
        at: DateTime<Utc>,
    ) -> ItemId {
        let id = self.ensure(id, turn, ItemKind::UserInput, at);
        let item = self
            .items
            .get_mut(&id)
            .expect("ensure inserted the user-input item");
        item.status = ItemStatus::Completed;
        item.text = Some(answer.to_string());
        id
    }

    fn apply_error(&mut self, seq: u64, message: &str, at: DateTime<Utc>) -> ItemId {
        let id = keyed_item_id(&format!("error:{seq}"));
        let turn = self.current_turn.unwrap_or_else(orphan_turn);
        let mut item = build(id, turn, ItemKind::Error, at);
        item.text = Some(message.to_string());
        item.status = ItemStatus::Failed;
        self.upsert(item)
    }

    /// Fold a terminal `turn_failed` into the durable `error` item of that
    /// turn. The id is deterministic on the turn id, so a replay (possibly
    /// after compaction or a restart) reproduces the same row the live stream
    /// appended; a duplicate fold keeps the first snapshot untouched.
    fn apply_turn_failed(
        &mut self,
        id: ItemId,
        turn: TurnId,
        message: &str,
        at: DateTime<Utc>,
    ) -> ItemId {
        let mut item = build(id, turn, ItemKind::Error, at);
        item.text = Some(message.to_string());
        item.status = ItemStatus::Failed;
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

    fn ensure(&mut self, id: ItemId, turn: TurnId, kind: ItemKind, at: DateTime<Utc>) -> ItemId {
        if !self.items.contains_key(&id) {
            self.order.push(id);
            self.items.insert(id, build(id, turn, kind, at));
        }
        id
    }

    fn upsert(&mut self, item: ThreadItem) -> ItemId {
        let id = item.item_id;
        if !self.items.contains_key(&id) {
            self.order.push(id);
        }
        self.items.insert(id, item);
        id
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
        projector.apply_in_place(event);
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
                item_id: None,
                timestamp: now,
                call_id: "call_1".into(),
                tool: "read".into(),
                arguments: serde_json::json!({ "path": "src/main.rs" }),
            },
            RuntimeEvent::ToolCallFinished {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                item_id: None,
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
                item_id: None,
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
    fn projected_events_use_the_supplied_item_id() {
        let turn = turn();
        let thread = ark_common::ThreadId::new();
        let now = Utc::now();
        let supplied = ItemId::new();
        let events = vec![
            RuntimeEvent::ToolCallStarted {
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                item_id: Some(supplied),
                timestamp: now,
                call_id: "call_1".into(),
                tool: "read".into(),
                arguments: serde_json::json!({ "path": "src/main.rs" }),
            },
            RuntimeEvent::UserInputRequested {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                item_id: Some(ItemId::new()),
                timestamp: now,
                request_id: "ui_1".into(),
                prompt: "which file?".into(),
            },
            RuntimeEvent::TurnFailed {
                seq: 3,
                thread_id: thread,
                turn_id: turn,
                item_id: Some(ItemId::new()),
                timestamp: now,
                message: "failed".into(),
                model_request_failed: false,
            },
        ];

        let items = project_items(&events);

        assert_eq!(items[0].item_id, supplied);
    }

    #[test]
    fn legacy_projected_events_fall_back_to_stable_ids() {
        let turn = turn();
        let thread = ark_common::ThreadId::new();
        let event = RuntimeEvent::ToolCallFinished {
            seq: 1,
            thread_id: thread,
            turn_id: turn,
            item_id: None,
            timestamp: Utc::now(),
            call_id: "legacy_call".into(),
            tool: "read".into(),
            ok: true,
            duration_ms: 1,
            output: "contents".into(),
            detail: None,
        };
        let json = serde_json::to_value(event).unwrap();
        assert!(json.get("item_id").is_none());
        let event: RuntimeEvent = serde_json::from_value(json).unwrap();

        let items = project_items(&[event]);

        assert_eq!(items[0].item_id, keyed_item_id("tool_call:legacy_call"));
    }

    #[test]
    fn errors_are_attributed_to_the_current_turn() {
        let turn = turn();
        let thread = ark_common::ThreadId::new();
        let now = Utc::now();
        let events = vec![
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: now,
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
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
    fn turn_failed_folds_into_one_durable_error_item() {
        // The terminal failure is the interruption point the renderer shows
        // inline: exactly one `error` item per failed turn, attributed to that
        // turn and replay-stable through the deterministic id.
        let turn = turn();
        let thread = ark_common::ThreadId::new();
        let now = Utc::now();
        let events = vec![
            RuntimeEvent::TurnStarted {
                auto_recovery: 0,
                seq: 1,
                thread_id: thread,
                turn_id: turn,
                timestamp: now,
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
            RuntimeEvent::TurnFailed {
                seq: 2,
                thread_id: thread,
                turn_id: turn,
                item_id: None,
                timestamp: now,
                message: "turn budget: max model rounds reached".into(),
                model_request_failed: false,
            },
        ];
        let items = project_items(&events);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ItemKind::Error);
        assert_eq!(items[0].turn_id, turn);
        assert_eq!(items[0].status, ItemStatus::Failed);
        assert_eq!(
            items[0].text.as_deref(),
            Some("turn budget: max model rounds reached")
        );

        // Replaying the same log (a restart rebuild) produces identical rows.
        let replayed = project_items(&events);
        assert_eq!(replayed, items);
    }

    #[test]
    fn turn_aborted_projects_no_error_item() {
        // An abort is a deliberate user action, not a failure: no error row.
        let turn = turn();
        let thread = ark_common::ThreadId::new();
        let now = Utc::now();
        let events = vec![RuntimeEvent::TurnAborted {
            seq: 1,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            reason: "user requested".into(),
        }];
        assert!(project_items(&events).is_empty());
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
