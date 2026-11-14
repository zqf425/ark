//! Turn-start thread naming: at the first explicit user turn, replace a
//! placeholder title with the head of the message itself — before the worker
//! runs, so every client that renders the thread title sees the real name from
//! the first streamed frame.
//!
//! Ownership: this module owns the *policy* (when to name, the placeholder
//! set, the byte budget). There is no model call: the turn runner's first
//! assistant reply already models the conversation, and naming from the head
//! of the user text keeps naming synchronous, deterministic and free of a
//! provider dependency. On any guard failure the thread simply keeps its
//! current title — naming is best effort and must never block or fail a turn.
//!
//! The rename goes through `SessionStore::rename_thread` plus the same
//! `ThreadUpdated` event the `PATCH /v1/threads/{id}` route writes, so every
//! client that can observe a manual rename (index refresh, SSE rail row) sees
//! an automatic one too — there is no second write path.

use ark_common::ThreadId;
use ark_contract::RuntimeEvent;
use ark_ports::SessionStore;
use chrono::Utc;

/// The first turn's text contributes at most this many bytes to the new title.
/// Long titles truncate in the UI rail today, so a short title is also a
/// rendering guarantee, not only a budget. The clamp cuts on a char boundary.
const TITLE_MAX_BYTES: usize = 40;

/// Titles the naming step is allowed to replace. `New thread` is the HTTP
/// create fallback, `Untitled thread` the store's normalization fallback, and
/// `新会话` the web renderer's localized default — all three mean "the user
/// did not name this thread yet".
const PLACEHOLDER_TITLES: [&str; 3] = ["New thread", "Untitled thread", "新会话"];

/// Whether a title is a placeholder the naming step may replace.
fn is_placeholder(title: &str) -> bool {
    let trimmed = title.trim();
    PLACEHOLDER_TITLES.contains(&trimmed)
}

/// Turn the first turn's text into the bounded title: collapse whitespace,
/// clamp onto the byte budget on a char boundary.
fn title_from_text(text: &str) -> String {
    let single_spaced = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let (clipped, _) = ark_ports::truncate_utf8(&single_spaced, TITLE_MAX_BYTES);
    clipped.trim().to_string()
}

/// Rename a still-placeholder thread from the first explicit user turn's text.
///
/// Called between turn registration and worker spawn in `start_turn_as`, so
/// the rename and its `ThreadUpdated` event are durable before the first model
/// token streams. Never fails the turn: every guard failure keeps the current
/// title and logs.
pub async fn name_thread_on_first_turn(store: &dyn SessionStore, thread_id: ThreadId, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let current = match store.get_thread(thread_id).await {
        Ok(meta) => meta.title,
        Err(error) => {
            tracing::warn!(
                thread = %thread_id,
                error = %error.message,
                "turn-start thread naming skipped: thread is unreadable"
            );
            return;
        }
    };
    if !is_placeholder(&current) {
        // A rename is user intent (or a policy that already named the thread,
        // e.g. IM's first-message title): it is never overwritten.
        return;
    }
    let title = title_from_text(text);
    if title.is_empty() {
        // Unreachable behind the non-empty-text guard; kept so the silent path
        // can never come back by accident.
        return;
    }
    if let Err(error) = store.rename_thread(thread_id, &title).await {
        tracing::warn!(
            thread = %thread_id,
            error = %error.message,
            "turn-start thread naming failed; keeping the current title"
        );
        return;
    }
    let event = RuntimeEvent::ThreadUpdated {
        seq: 0,
        thread_id,
        timestamp: Utc::now(),
        title: title.clone(),
    };
    if let Err(error) = store.append_event(thread_id, event).await {
        tracing::warn!(
            thread = %thread_id,
            error = %error.message,
            "turn-start thread naming applied but the update event failed"
        );
        return;
    }
    tracing::info!(thread = %thread_id, %title, "thread named from the first turn");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end with the real store behind the port: a placeholder thread
    /// gets renamed and a `thread_updated` event lands in its log; a
    /// user-renamed thread is left alone.
    #[tokio::test]
    async fn naming_renames_a_placeholder_thread_before_the_reply() {
        let dir = tempfile::Builder::new()
            .prefix("ark-titles-")
            .tempdir()
            .expect("temp dir");
        let store = ark_store::FileSessionStore::open(ark_store::StoreConfig::new(dir.path()))
            .expect("session store");
        let thread_id = store
            .create_thread("新会话")
            .await
            .expect("thread")
            .thread_id;

        name_thread_on_first_turn(&store, thread_id, "  帮我评审这段   Rust 代码  \n").await;
        let renamed = store.get_thread(thread_id).await.expect("thread");
        assert_eq!(renamed.title, "帮我评审这段 Rust 代码");

        let events = store.replay_events(thread_id, 0).await.expect("events");
        let last = events.last().expect("a rename event");
        assert!(matches!(
            last,
            RuntimeEvent::ThreadUpdated { title, .. } if title == "帮我评审这段 Rust 代码"
        ));

        // A thread the user already renamed is never re-titled.
        store
            .rename_thread(thread_id, "我的名字")
            .await
            .expect("rename");
        name_thread_on_first_turn(&store, thread_id, "再来一轮").await;
        assert_eq!(
            store.get_thread(thread_id).await.expect("thread").title,
            "我的名字"
        );
    }
}
