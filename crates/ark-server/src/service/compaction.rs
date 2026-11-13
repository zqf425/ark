//! Explicit compaction orchestration (design §7.1 compaction semantics).
//!
//! The checkpoint *rendering* lives in `ark-agent`; this module rebuilds it
//! from the durable items and persists a fresh notice. Kept out of the HTTP
//! handler so the route stays transport-only.

use std::sync::Arc;

use ark_common::{ThreadId, TurnId};
use ark_contract::{ItemKind, ItemMarker, ThreadItem};
use ark_ports::{ModelMessage, SessionStore};

/// What one compaction invocation did.
#[derive(Debug)]
pub enum CompactionOutcome {
    /// A fresh checkpoint notice was created for `turn_id`.
    Created { turn_id: TurnId, checkpoint: String },
}

/// Rebuild and persist a checkpoint from the thread's durable items.
///
/// Always appends a *fresh* boundary: `conversation_history` replays only the
/// rows after the newest checkpoint, so a new note is what actually advances
/// the boundary. Returning the existing note would leave manual compaction a
/// no-op forever. The old rows stay on disk for rewind / export.
pub async fn ensure_checkpoint(
    store: &Arc<dyn SessionStore>,
    thread_id: ThreadId,
) -> ark_common::Result<CompactionOutcome> {
    store.get_thread(thread_id).await?;
    let items = store.read_items(thread_id).await?;
    // Preserve roles where the item carries them so user requests and failure
    // evidence get the same recall as the in-turn compact path. Tool output
    // lives in the payload, not `text`.
    let messages: Vec<ModelMessage> = items
        .iter()
        .filter_map(|item| {
            let text = item
                .text
                .as_deref()
                .or_else(|| item.tool.as_ref()?.output.as_deref())?;
            Some(match item.kind {
                ItemKind::UserMessage => ModelMessage::user(text),
                ItemKind::AssistantMessage => ModelMessage::assistant(text),
                _ => ModelMessage::system(text),
            })
        })
        .collect();
    let checkpoint = ark_agent::ContextCheckpoint::from_messages(&messages).render();
    let turn_id = TurnId::new();
    let item = ThreadItem::new(turn_id, ItemKind::Notice)
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
