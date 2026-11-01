//! Attachment association projection for the HTTP boundary.
//!
//! Association is deliberately *not* stored: the durable `attachment_associated`
//! event is the only authority, so a replay after a restart restores exactly the
//! authorizations that were granted. These helpers are the one place that folds
//! those events into the sets the routes compare against, which keeps thread
//! detail, turn-start authorization and the delete guard from each inventing a
//! slightly different rule.

use std::collections::HashSet;

use ark_common::{AttachmentId, ThreadId};
use ark_contract::RuntimeEvent;

use crate::api::error::ApiError;
use crate::state::AppState;

/// Attachment ids one thread has authorized, in durable log order.
pub async fn thread_attachment_ids(
    state: &AppState,
    thread_id: ThreadId,
) -> Result<Vec<AttachmentId>, ApiError> {
    let events = state.store.replay_attachment_events(thread_id).await?;
    Ok(attachment_ids(&events))
}

/// Attachment ids authorized by any thread.
///
/// Used by the delete guard and by `orphan_count`: an id associated anywhere is
/// in use, so it neither may be deleted nor counts as an orphan.
pub async fn associated_attachment_ids(
    state: &AppState,
) -> Result<HashSet<AttachmentId>, ApiError> {
    let mut ids = HashSet::new();
    for meta in state.store.list_threads().await? {
        ids.extend(thread_attachment_ids(state, meta.thread_id).await?);
    }
    Ok(ids)
}

/// Fold association events into a deduplicated id list.
///
/// Duplicates are absorbed here rather than by a projection pass: the write path
/// already refuses to append a second event for the same pair, so a repeat can
/// only come from an older log, and readers must not see it twice.
pub fn attachment_ids(events: &[RuntimeEvent]) -> Vec<AttachmentId> {
    let mut ids: Vec<AttachmentId> = Vec::new();
    for event in events {
        if let RuntimeEvent::AttachmentAssociated { attachment_id, .. } = event {
            if !ids.contains(attachment_id) {
                ids.push(*attachment_id);
            }
        }
    }
    ids
}

/// Reject attachment ids this thread never authorized.
///
/// The design fixes one edge: a turn may only reference attachments associated
/// with *its own* thread. An attachment that exists but belongs to another
/// thread answers the same 404 as a missing one, so the error cannot be used to
/// probe which ids exist.
pub async fn authorize_turn_attachments(
    state: &AppState,
    thread_id: ThreadId,
    requested: Vec<AttachmentId>,
) -> Result<Vec<AttachmentId>, ApiError> {
    if requested.is_empty() {
        return Ok(Vec::new());
    }
    let associated = thread_attachment_ids(state, thread_id).await?;
    let mut allowed: Vec<AttachmentId> = Vec::new();
    for id in requested {
        if !associated.contains(&id) {
            return Err(ApiError::new(
                ark_common::ArkErrorCode::AttachmentNotAssociated,
                format!("attachment {id} is not associated with thread {thread_id}"),
            ));
        }
        // The wire contract is a set: repeating one id never means "use it
        // twice", and a duplicate in the journal would inflate later readers.
        if !allowed.contains(&id) {
            allowed.push(id);
        }
    }
    Ok(allowed)
}
