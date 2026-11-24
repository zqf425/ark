//! Explicit compaction orchestration (design §7.1 compaction semantics).
//!
//! The checkpoint *rendering* lives in `ark-agent`; this module rebuilds it
//! from the durable items and persists a fresh notice. Kept out of the HTTP
//! handler so the route stays transport-only.

use std::sync::Arc;

use ark_common::{ThreadId, TurnId};
use ark_contract::{ItemKind, ItemMarker, ThreadItem, ThreadRuntimeSettings};
use ark_ports::SessionStore;

use crate::state::AppState;

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
    state: &AppState,
    thread_id: ThreadId,
) -> ark_common::Result<CompactionOutcome> {
    let workspace_guard = state.workspace_gate.lock().await;
    let Some(_reservation) = state.turns.reserve_compaction(thread_id) else {
        return Err(ark_common::ArkError::new(
            ark_common::ArkErrorCode::Conflict,
            "thread already has a running turn or compaction",
        ));
    };
    drop(workspace_guard);

    let thread = state.store.get_thread(thread_id).await?;
    let threshold = manual_compaction_threshold(state, &thread.runtime);
    rebuild_checkpoint(&state.store, thread_id, threshold).await
}

async fn rebuild_checkpoint(
    store: &Arc<dyn SessionStore>,
    thread_id: ThreadId,
    context_limit_tokens: u64,
) -> ark_common::Result<CompactionOutcome> {
    let items = store.read_items(thread_id).await?;
    let messages = ark_agent::model_history(&items);
    let checkpoint = ark_agent::ContextCheckpoint::from_compaction_history(&messages);
    let checkpoint = checkpoint.render_with_token_limit(context_limit_tokens.saturating_sub(1_024));
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

fn manual_compaction_threshold(state: &AppState, runtime: &ThreadRuntimeSettings) -> u64 {
    const DEFAULT_WINDOW: u64 = 256_000;
    let Some(providers) = &state.base.providers else {
        return ark_common::compaction_threshold(DEFAULT_WINDOW, 0);
    };
    let Some(endpoint) = providers.resolve_endpoint(runtime.provider_id.as_deref()) else {
        return ark_common::compaction_threshold(DEFAULT_WINDOW, 0);
    };
    let model = runtime.model.as_deref().unwrap_or(&endpoint.model);
    let provider = providers
        .list()
        .providers
        .into_iter()
        .find(|provider| provider.id == endpoint.provider_id);
    let window = provider
        .as_ref()
        .and_then(|provider| {
            provider
                .models
                .iter()
                .find(|entry| entry.id == model)
                .and_then(|entry| entry.context_window_tokens)
                .or(provider.context_window_tokens)
        })
        .unwrap_or(DEFAULT_WINDOW);
    ark_common::compaction_threshold(window, u64::from(endpoint.max_tokens.unwrap_or(0)))
}
