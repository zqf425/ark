//! Thread creation orchestration shared by the HTTP boundary and the scheduler.
//!
//! `POST /v1/threads` and a scheduled run must produce the same two records in
//! the same order: a scheduled run that skipped the `thread_created` event would
//! create a thread an attached SSE client never learns about, which is the one
//! difference the renderer cannot compensate for.

use ark_common::Result;
use ark_contract::{RuntimeEvent, ThreadSummary};
use ark_ports::ThreadMeta;
use chrono::Utc;

use crate::state::AppState;

/// Create a thread and publish its `thread_created` event.
pub async fn create_thread(state: &AppState, title: &str) -> Result<ThreadSummary> {
    let meta = state.store.create_thread(title).await?;
    // Publish the creation so an SSE client attached to the new thread sees it
    // without a special case in the stream handler.
    let event = RuntimeEvent::ThreadCreated {
        seq: 0,
        thread_id: meta.thread_id,
        timestamp: Utc::now(),
        title: meta.title.clone(),
    };
    state.store.append_event(meta.thread_id, event).await?;
    let mut summary = summary_of(&meta);
    summary.last_seq = state.store.highest_seq(meta.thread_id).await.ok();
    Ok(summary)
}

/// Map stored metadata onto the wire shape.
pub fn summary_of(meta: &ThreadMeta) -> ThreadSummary {
    ThreadSummary {
        thread_id: meta.thread_id,
        title: meta.title.clone(),
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        // The store derives the durable watermark from the index/log; the
        // list, detail and fork rows must all carry it so the SSE client never
        // subscribes from 0 and folds the seeded history a second time.
        last_seq: meta.last_seq,
        forked_from: meta.forked_from,
        fork_at_seq: meta.fork_at_seq,
        inherited_item_count: meta.inherited_item_count,
        archived: meta.archived,
    }
}
