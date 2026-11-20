//! Thread creation orchestration shared by the HTTP boundary and the scheduler.
//!
//! `POST /v1/threads` and a scheduled run must produce the same two records in
//! the same order: a scheduled run that skipped the `thread_created` event would
//! create a thread an attached SSE client never learns about, which is the one
//! difference the renderer cannot compensate for.

use ark_common::path::normalize;
use ark_common::{ArkErrorCode, Result, ThreadId};
use ark_contract::{BulkDeleteSkipped, BulkDeleteThreadsResponse, RuntimeEvent, ThreadSummary};
use ark_ports::ThreadMeta;
use chrono::Utc;
use std::collections::HashSet;

use crate::state::AppState;

/// Create a thread and publish its `thread_created` event.
///
/// `workspace` is the client's per-thread workspace for the new session (the
/// rail's per-project "new session" action); `None` falls back to the
/// configured default workspace. Validation of an explicit path happens in the
/// HTTP boundary, which owns the workspace service seam; other callers
/// (scheduler, IM) pass `None` and keep today's seeding.
pub async fn create_thread(
    state: &AppState,
    title: &str,
    workspace: Option<&str>,
) -> Result<ThreadSummary> {
    let mut meta = state.store.create_thread(title).await?;
    // A new thread starts in the configured default workspace (or the caller's
    // explicit override), so the composer shows a real project instead of
    // （未设置） without a manual switch. The seed happens before the
    // `thread_created` event so an SSE client attached to the new thread never
    // sees a workspace-less snapshot; an explicit thread-level workspace
    // (fork inheritance, IM/scheduler callers) wins. Both branches normalize
    // with the same single implementation the runtime uses for comparisons
    // (`ark_common::path::normalize`) — the configured default is often a
    // native backslash path, and an un-normalized seed would group as a
    // separate project from the same root seeded through an explicit
    // (already normalized) create call.
    if meta.runtime.workspace.is_none() {
        let seeded = workspace
            .map(normalize)
            .or_else(|| {
                state
                    .config
                    .snapshot()
                    .default_workspace
                    .as_deref()
                    .map(|path| normalize(&path.to_string_lossy()))
            })
            .filter(|path| !path.is_empty());
        if let Some(workspace) = seeded {
            meta.runtime.workspace = Some(workspace);
            meta = state
                .store
                .set_thread_runtime(meta.thread_id, meta.runtime.clone())
                .await?;
        }
    }
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

/// Resolve threads' durable "turn in flight" verdict.
///
/// The turn registry is process-local truth: a restart drops it, and turns
/// started by another process (IM channel, scheduled run in a separate
/// renderer) never register here. Mirrors the busy rule — a thread
/// reads busy when its newest durable turn-lifecycle event has not reached a
/// terminal state (`turn_completed` / `turn_failed` / `turn_aborted`) — so the
/// marker survives restarts. Only threads outside the registry's active set
/// are probed: an active registry entry is already authoritative and the log
/// fold stays off the hot path.
///
/// Shared by the list route (busy markers) and the per-project bulk delete
/// (which threads are settled), so both can never disagree about "running".
pub async fn durable_running_ids(
    state: &AppState,
    registry_active: &HashSet<ThreadId>,
    candidates: impl IntoIterator<Item = ThreadId>,
) -> HashSet<ThreadId> {
    let mut running = HashSet::new();
    for thread_id in candidates {
        if registry_active.contains(&thread_id) {
            running.insert(thread_id);
            continue;
        }
        // `interaction_events` is the bounded turn/gate vocabulary, so this
        // fold is cheap even for long logs. A probe failure must not break
        // the caller: the registry verdict (idle) stands.
        let events = match state.store.replay_interaction_events(thread_id).await {
            Ok(events) => events,
            Err(_) => continue,
        };
        // The *newest* turn-lifecycle event decides, the same fold
        // `interrupted_user_lifecycle` uses: gate records are skipped, and
        // `any()` would be wrong because an older `turn_started` behind a
        // newer terminal record must not resurrect a settled turn.
        let verdict = events
            .iter()
            .rev()
            .find(|event| {
                matches!(
                    event,
                    RuntimeEvent::TurnStarted { .. }
                        | RuntimeEvent::TurnCompleted { .. }
                        | RuntimeEvent::TurnFailed { .. }
                        | RuntimeEvent::TurnAborted { .. }
                )
            })
            .is_some_and(|event| matches!(event, RuntimeEvent::TurnStarted { .. }));
        if verdict {
            running.insert(thread_id);
        }
    }
    running
}

/// Map stored metadata onto the wire shape.
///
/// The workspace string is the rail's exact-match grouping key, so the read
/// boundary normalizes it too: legacy rows and side-door writers (scheduler,
/// IM threads) may still hold a native backslash form in `thread.json`, and a
/// verbatim projection would keep that directory split into two project
/// groups even after every write path stores the normalized form.
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
        // The list groups threads into projects by workspace; `to_meta` fills
        // this from `thread.json`, the index path mirrors it for list rows.
        workspace: meta
            .runtime
            .workspace
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(normalize),
        archived: meta.archived,
        // `running` is live registry state only the caller knows; the mapping
        // from metadata alone cannot assert it.
        running: false,
        last_event_at: meta.last_event_at,
    }
}

/// `DELETE /v1/threads?workspace=…` orchestration: delete every **settled**
/// thread whose effective workspace matches, keep everything else.
///
/// The single-thread delete handler (`api::handlers::threads::remove`) stays
/// the authority on one thread's deletion semantics (running-turn refusal,
/// continuation suppression, tombstone reclamation); this bulk path only
/// decides *which* threads qualify and reports per-thread refusals instead of
/// failing the whole request. Matching follows the rail's grouping rule
/// (`thread-projects.ts`): an explicit override compares against the needle,
/// and a thread without one falls back to the runtime's active root, so the
/// UI's active project group and this verb can never disagree about membership.
pub async fn delete_settled_by_workspace(
    state: &AppState,
    workspace: &str,
) -> Result<BulkDeleteThreadsResponse> {
    let needle = normalize(workspace);
    // Same fallback grouping key as the renderer's rail (see the `workspace`
    // field on `ThreadSummary`): no override means "the active root".
    let active_fallback = state
        .base
        .workspace
        .as_ref()
        .and_then(|service| service.view().active)
        .map(|descriptor| normalize(&descriptor.path));
    let metas = state.store.list_threads().await?;
    let registry_active = state.turns.active_thread_ids();
    // One durable fold for the whole sweep: a thread whose newest turn
    // event is still `turn_started` counts as running even when the registry
    // forgot it (restart, another process). Same verdict as the list markers.
    let ids: Vec<ThreadId> = metas.iter().map(|meta| meta.thread_id).collect();
    let running = durable_running_ids(state, &registry_active, ids).await;
    let mut response = BulkDeleteThreadsResponse::default();
    for meta in metas {
        let matches = match meta
            .runtime
            .workspace
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            Some(path) => normalize(path) == needle,
            // The active root itself is the fallback key; the default group
            // (no active root and no override) is deliberately not served.
            None => active_fallback.as_deref() == Some(needle.as_str()),
        };
        if !matches {
            continue;
        }
        // Running threads are per-thread skip results, not a failed request:
        // the user asked to clean finished sessions, so finishing ones survive.
        if running.contains(&meta.thread_id) {
            response.skipped.push(BulkDeleteSkipped {
                thread_id: meta.thread_id,
                code: ArkErrorCode::Conflict,
            });
            continue;
        }
        match delete_settled(state, meta.thread_id).await {
            Ok(()) => response.deleted.push(meta.thread_id),
            Err(error) => response.skipped.push(BulkDeleteSkipped {
                thread_id: meta.thread_id,
                code: error.code,
            }),
        }
    }
    Ok(response)
}

/// Delete one thread that has no in-flight registry turn, mirroring the
/// single-thread handler's durable steps (suppress → delete → reclaim).
///
/// Ownership existence is re-checked here so a concurrent delete between the
/// listing and this call reports a structured 404-shaped skip instead of
/// inventing a second row.
async fn delete_settled(state: &AppState, thread_id: ThreadId) -> Result<()> {
    state.store.get_thread(thread_id).await?;
    let was_suppressed = state.continuations.suppress(thread_id);
    if let Err(error) = state.store.delete_thread(thread_id).await {
        if !was_suppressed {
            state.continuations.allow(thread_id);
            crate::service::schedule_auto_continuation(state.clone(), thread_id).await;
        }
        return Err(error);
    }
    // Thread ids are never reused: reclaim the process-local tombstone and the
    // turn-registry epoch marker exactly like the single delete route.
    state.continuations.allow(thread_id);
    state.turns.forget_thread(thread_id);
    Ok(())
}
