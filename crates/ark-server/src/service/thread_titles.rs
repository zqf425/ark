//! Auto thread titles: after the first explicit user turn completes, replace a
//! placeholder title with a short model-generated one.
//!
//! Ownership: this module owns the *policy* (when to name, the placeholder set,
//! the prompt, the length budget, the timeout); the *model call* is injected
//! through the `BaseServices::title_model` port, because only the composition
//! root (`arkd`) knows which provider is live. On any failure the thread simply
//! keeps its current title — naming is best effort and must never affect the
//! completed turn it observes.
//!
//! The rename goes through `SessionStore::rename_thread` plus the same
//! `ThreadUpdated` event the `PATCH /v1/threads/{id}` route writes, so every
//! client that can observe a manual rename (index refresh, SSE rail row) sees
//! an automatic one too — there is no second write path.

use ark_common::{Result, ThreadId};
use ark_contract::RuntimeEvent;
use chrono::Utc;

use crate::state::AppState;

/// The first turn's text contributes at most this many bytes to the naming
/// prompt. A longer message is truncated on a char boundary; the model only
/// needs the gist, and the budget keeps the prompt bounded.
const PROMPT_EXCERPT_BYTES: usize = 2000;

/// Generated titles are clamped to this many UTF-8 bytes on a char boundary.
/// Long titles truncate in the UI rail today, so a short title is also a
/// rendering guarantee, not only a budget.
const TITLE_MAX_BYTES: usize = 40;

/// How long the background naming call may take. Generous enough for a slow
/// provider, small enough that shutdown drain is never visibly delayed: the
/// task is awaited only to log its outcome.
const TITLE_TIMEOUT_SECS: u64 = 15;

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

/// Turn the first turn's text into the bounded naming excerpt.
fn excerpt_of(text: &str) -> String {
    let (excerpt, _) = ark_ports::truncate_utf8(text.trim(), PROMPT_EXCERPT_BYTES);
    excerpt.to_string()
}

/// Clamp a generated title onto the byte budget and collapse its whitespace.
fn clamp_title(raw: &str) -> String {
    let single_spaced = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let (clipped, _) = ark_ports::truncate_utf8(single_spaced.trim(), TITLE_MAX_BYTES);
    clipped.trim().to_string()
}

/// The naming instruction. ASCII only: the excerpt carries the language, and
/// the model answers in the conversation's own language. No character budget
/// is named here — the byte clamp after the call is the single enforcement
/// point, so the prompt and the clamp can never disagree.
fn title_prompt(excerpt: &str) -> String {
    format!(
        "Generate a very short title (a few words, no quotes, no punctuation at the end) \
for the following conversation opening. Answer with the title text only.\n\n{excerpt}"
    )
}

/// Spawn the background naming step for a completed explicit first turn.
///
/// Called by the turn supervisor once the reactor reached `Completed`. The task
/// is detached on purpose: a slow provider must not hold the supervisor task
/// (and with it the turn's own bookkeeping) open. Detached does not mean
/// unsupervised — every failure is logged, and the store write is the same
/// atomic rename the manual route uses.
pub fn schedule_title_generation(state: AppState, thread_id: ThreadId, first_turn_text: String) {
    tokio::spawn(async move {
        let excerpt = excerpt_of(&first_turn_text);
        match generate_title(&state, &excerpt).await {
            Ok(Some(title)) => {
                if let Err(error) = apply_title(&state, thread_id, &title).await {
                    tracing::warn!(
                        thread = %thread_id,
                        error = %error.message,
                        "auto thread title was generated but not stored"
                    );
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    thread = %thread_id,
                    error = %error.message,
                    "auto thread title generation failed; keeping the current title"
                );
            }
        }
    });
}

/// Run the naming model call under the timeout budget. `Ok(None)` means no
/// title model is wired; the caller then keeps the placeholder.
async fn generate_title(state: &AppState, excerpt: &str) -> Result<Option<String>> {
    let Some(model) = state.base.title_model.as_ref() else {
        return Ok(None);
    };
    let prompt = title_prompt(excerpt);
    let call = model.generate_title(&prompt);
    match tokio::time::timeout(std::time::Duration::from_secs(TITLE_TIMEOUT_SECS), call).await {
        Ok(outcome) => outcome.map(|generated| generated.map(|title| clamp_title(&title))),
        // A timeout is just a failed call: the title stays as it is. It must
        // not share the silent `Ok(None)` of "no model wired" — a provider
        // that never answers within the budget would otherwise be invisible.
        Err(_) => {
            tracing::warn!(
                budget_secs = TITLE_TIMEOUT_SECS,
                "auto thread title generation timed out; keeping the current title"
            );
            Ok(None)
        }
    }
}

/// Store the generated title with the manual-rename event shape.
async fn apply_title(state: &AppState, thread_id: ThreadId, title: &str) -> Result<()> {
    if title.is_empty() {
        // Silence here made a reasoning model's budget-exhausted empty answer
        // indistinguishable from "the feature ran and succeeded". Name it.
        tracing::warn!(
            thread = %thread_id,
            "auto thread title produced an empty title; keeping the placeholder"
        );
        return Ok(());
    }
    let meta = state.store.rename_thread(thread_id, title).await?;
    let event = RuntimeEvent::ThreadUpdated {
        seq: 0,
        thread_id,
        timestamp: Utc::now(),
        title: meta.title.clone(),
    };
    state.store.append_event(thread_id, event).await?;
    tracing::info!(thread = %thread_id, title = %meta.title, "auto thread title applied");
    Ok(())
}

/// Decide whether the completed turn should trigger naming, and return the
/// excerpt to name from.
///
/// Only an explicit user turn names a thread: auto-continuations and IM
/// messages do not own the composer's first input (IM threads already derive a
/// title from the first message in `im_inbound`). The thread must still carry a
/// placeholder title — a rename is user intent and is never overwritten.
pub async fn title_task_for(state: &AppState, thread_id: ThreadId, text: &str) -> Option<String> {
    let title = state
        .store
        .get_thread(thread_id)
        .await
        .map(|meta| meta.title)
        .unwrap_or_default();
    if text.trim().is_empty() || !is_placeholder(&title) {
        return None;
    }
    Some(excerpt_of(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, TurnEnd, TurnJob, TurnRunner};
    use std::sync::Arc;

    struct EmptyTools;

    impl ark_ports::ToolRegistryPort for EmptyTools {
        fn names(&self) -> Vec<String> {
            Vec::new()
        }

        fn schemas(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }

        fn version(&self) -> u64 {
            0
        }
    }

    struct ImmediateRunner;

    #[async_trait::async_trait]
    impl TurnRunner for ImmediateRunner {
        async fn run_turn(&self, _job: TurnJob) -> Result<TurnEnd> {
            Ok(TurnEnd::Completed)
        }
    }

    #[test]
    fn placeholder_detection_covers_the_three_fallbacks() {
        assert!(is_placeholder("New thread"));
        assert!(is_placeholder(" 新会话 "));
        assert!(is_placeholder("Untitled thread"));
        assert!(!is_placeholder("Rust 设计评审"));
        assert!(!is_placeholder(""));
    }

    #[test]
    fn excerpt_is_trimmed_and_char_boundary_safe() {
        let long = "你".repeat(PROMPT_EXCERPT_BYTES);
        let excerpt = excerpt_of(&format!("  {long}  "));
        assert!(excerpt.len() <= PROMPT_EXCERPT_BYTES);
        assert!(excerpt.is_char_boundary(excerpt.len()));
    }

    #[test]
    fn clamp_collapses_whitespace_and_cuts_on_char_boundary() {
        assert_eq!(clamp_title("  a  b  "), "a b");
        let multibyte = "你".repeat(TITLE_MAX_BYTES + 10);
        let clamped = clamp_title(&multibyte);
        assert!(clamped.len() <= TITLE_MAX_BYTES);
        assert!(clamped.chars().all(|ch| ch == '你'));
    }

    #[test]
    fn prompt_carries_the_excerpt() {
        let prompt = title_prompt("hello world");
        assert!(prompt.contains("hello world"));
        assert!(prompt.contains("title"));
    }

    /// End-to-end with the real store behind the port: a placeholder thread
    /// gets renamed and a `thread_updated` event lands in its log; a
    /// user-renamed thread is left alone.
    #[tokio::test]
    async fn naming_renames_a_placeholder_thread_and_publishes_the_update() {
        use ark_ports::SessionStore;

        struct YesTitle;
        #[async_trait::async_trait]
        impl ark_ports::TitleModel for YesTitle {
            async fn generate_title(&self, _excerpt: &str) -> Result<Option<String>> {
                Ok(Some("  Rust  设计   评审 ".to_string()))
            }
        }

        let dir = tempfile::Builder::new()
            .prefix("ark-titles-")
            .tempdir()
            .expect("temp dir");
        let sessions = ark_store::FileSessionStore::open(ark_store::StoreConfig::new(dir.path()))
            .expect("session store");
        let store: Arc<dyn SessionStore> = Arc::new(sessions);
        let state = AppState::new(
            Arc::clone(&store),
            Arc::new(EmptyTools),
            Arc::new(ark_config::ConfigStore::new(
                ark_config::RuntimeConfig::default(),
            )),
            Arc::new(ark_obs::UsageRecorder::new()),
            Arc::new(ImmediateRunner),
            dir.path().to_path_buf(),
            "test",
        )
        .with_base_services(ark_ports::BaseServices::new().with_title_model(Arc::new(YesTitle)));
        let thread = state.store.create_thread("新会话").await.expect("thread");

        let task = title_task_for(&state, thread.thread_id, "帮我评审这段 Rust 代码")
            .await
            .expect("a placeholder thread names itself");
        schedule_title_generation(state.clone(), thread.thread_id, task);
        // The naming decision and the store write are separate awaits on the
        // runtime; yielding lets the detached task finish its (already
        // resolved) call before we read the store.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let renamed = state
            .store
            .get_thread(thread.thread_id)
            .await
            .expect("thread");
        assert_eq!(renamed.title, "Rust 设计 评审");

        let events = state
            .store
            .replay_events(thread.thread_id, 0)
            .await
            .expect("events");
        let last = events.last().expect("a rename event");
        assert!(matches!(
            last,
            RuntimeEvent::ThreadUpdated { title, .. } if title == "Rust 设计 评审"
        ));

        // A thread the user already renamed is never re-titled.
        state
            .store
            .rename_thread(thread.thread_id, "我的名字")
            .await
            .expect("rename");
        assert!(
            title_task_for(&state, thread.thread_id, "再来一轮")
                .await
                .is_none(),
            "a user rename must be preserved"
        );
    }
}
