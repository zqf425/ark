//! Explicit compaction orchestration (design §7.1 compaction semantics).
//!
//! The checkpoint *rendering* lives in `ark-agent`; this module only decides
//! whether one already exists and persists the synthetic notice item when it
//! does not. Kept out of the HTTP handler so the route stays transport-only.

use std::sync::Arc;

use ark_common::{ThreadId, TurnId};
use ark_contract::{ItemMarker, ThreadItem};
use ark_ports::SessionStore;

/// What one compaction invocation did.
#[derive(Debug)]
pub enum CompactionOutcome {
    /// A checkpoint item was already present; nothing was appended.
    Existing(ThreadItem),
    /// A new checkpoint notice was created for `turn_id`.
    Created { turn_id: TurnId, checkpoint: String },
}

/// Find the thread's checkpoint item, or build and persist one.
pub async fn ensure_checkpoint(
    store: &Arc<dyn SessionStore>,
    thread_id: ThreadId,
) -> ark_common::Result<CompactionOutcome> {
    store.get_thread(thread_id).await?;
    let items = store.read_items(thread_id).await?;
    if let Some(existing) = items.iter().find(|item| item.is_context_checkpoint()) {
        return Ok(CompactionOutcome::Existing(existing.clone()));
    }
    let checkpoint = ark_agent::ContextCheckpoint::from_texts(
        items.iter().filter_map(|item| item.text.as_deref()),
    )
    .render();
    let turn_id = TurnId::new();
    let item = ThreadItem::new(turn_id, ark_contract::item::ItemKind::Notice)
        .with_text(checkpoint.clone())
        .with_marker(ItemMarker::ContextCheckpoint)
        .completed();
    store
        .append_event(
            thread_id,
            ark_contract::RuntimeEvent::ItemCreated {
                seq: 0,
                thread_id,
                turn_id,
                timestamp: chrono::Utc::now(),
                item,
            },
        )
        .await?;
    Ok(CompactionOutcome::Created {
        turn_id,
        checkpoint,
    })
}
