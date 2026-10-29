//! `POST /v1/threads/{id}/review` - a real code-review turn.
//!
//! The review is not a static placeholder: it starts an actual agent turn with
//! the same model/provider/permissions the thread already uses, seeded with the
//! thread's authoritative workspace-change projection. Conflict and unknown
//! thread semantics come from the shared [`start_turn`] core.
//!
//! This file is deliberately ASCII-only: its earlier revision lost Chinese
//! strings to an encoding round-trip, and the review instruction does not need
//! non-ASCII text to work.

use ark_contract::{ReviewRequest, TurnSummary, WorkspaceChangesResponse};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::api::error::ApiError;
use crate::api::handlers::threads::parse_thread;
use crate::api::handlers::turns::start_turn;
use crate::service::workbench;
use crate::state::AppState;

/// Upper bound for the change list embedded in the review prompt.
const MAX_CHANGE_LIST_BYTES: usize = 8 * 1024;

/// `POST /v1/threads/{id}/review`.
///
/// Accepts an optional `{ "focus": "..." }` body. The response is the same
/// `202` + `TurnSummary` envelope as a normal turn start: the review's progress
/// is traceable through the thread's event stream under the returned turn id.
///
/// The change projection is a near-time snapshot taken before the turn
/// registers: if another turn is writing the workspace, this request either
/// answers 409 or sees a slightly stale projection. The model can re-read
/// current state with its tools, so freshness is advisory, not authoritative.
pub async fn review(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<TurnSummary>), ApiError> {
    let thread_id = parse_thread(&id)?;
    // The body is optional: an empty body means "no focus". Parsing happens
    // explicitly so an empty POST cannot fail before the handler runs.
    let request: ReviewRequest = if body.is_empty() {
        ReviewRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|_| {
            ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                "review body must be a valid ReviewRequest JSON object",
            )
        })?
    };
    let focus = request.focus;

    // The prompt seeds from the thread's authoritative change projection, so the
    // review always looks at what this thread actually did to the workspace.
    let events = state.store.replay_events(thread_id, 0).await?;
    let changes: WorkspaceChangesResponse = workbench::changes(&events);
    let prompt = review_prompt(&changes, focus.as_deref());

    start_turn(&state, thread_id, prompt).await
}

/// Compose the bounded review instruction from the change projection.
fn review_prompt(changes: &WorkspaceChangesResponse, focus: Option<&str>) -> String {
    let mut prompt = String::from(
        "Perform a code review of the uncommitted workspace changes produced \
         by this thread. Go file by file: describe problems, risks, and \
         improvement suggestions, then give an overall conclusion.\n\
         Change list of this thread (path + operation):\n",
    );
    for change in &changes.changes {
        let line = format!("- {} ({})\n", change.path, change.operation);
        if prompt.len() + line.len() > MAX_CHANGE_LIST_BYTES {
            prompt.push_str("- ... (remaining changes truncated; use the read tool as needed)\n");
            break;
        }
        prompt.push_str(&line);
    }
    if changes.changes.is_empty() {
        prompt.push_str(
            "(No workspace changes were recorded in the event log; inspect the \
             current workspace state and state your conclusion.)\n",
        );
    }
    if let Some(focus) = focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        prompt.push_str("\nReview focus: ");
        prompt.push_str(focus);
        prompt.push('\n');
    }
    prompt
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use ark_common::Result;
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig, StoreLimits};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::Utc;
    use tower::ServiceExt;

    use super::*;
    use crate::api::routes::router;
    use crate::state::{AppState, TurnEnd, TurnJob, TurnRunner};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ark-review-test-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().expect("system clock")
            ));
            std::fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct EmptyTools;

    impl ToolRegistryPort for EmptyTools {
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

    /// A runner that never settles: keeps the turn registered so the conflict
    /// path can be exercised.
    struct PendingRunner;

    #[async_trait::async_trait]
    impl TurnRunner for PendingRunner {
        async fn run_turn(&self, _job: TurnJob) -> Result<TurnEnd> {
            std::future::pending().await
        }
    }

    fn app(dir: &TempDir) -> (axum::Router, Arc<FileSessionStore>) {
        let store = Arc::new(
            FileSessionStore::open(
                StoreConfig::new(dir.path()).with_limits(StoreLimits::for_tests()),
            )
            .expect("open test store"),
        );
        let state = AppState::new(
            store.clone(),
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(PendingRunner),
            dir.path().to_path_buf(),
            "test",
        );
        (router(state), store)
    }

    #[tokio::test]
    async fn review_starts_a_real_turn_and_conflicts_while_one_is_running() {
        let dir = TempDir::new();
        let (app, store) = app(&dir);
        let created = store
            .create_thread("review-me")
            .await
            .expect("create thread");
        let base = format!("/v1/threads/{}", created.thread_id);

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{base}/review"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"focus":"src/api"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(first.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(body["turn_id"].is_string());

        let second = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{base}/review"))
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn review_answers_structured_not_found_for_unknown_threads() {
        let dir = TempDir::new();
        let (app, _store) = app(&dir);
        let missing = ark_common::ThreadId::new();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/threads/{missing}/review"))
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn review_prompt_stays_ascii_and_bounded() {
        let changes = WorkspaceChangesResponse {
            changes: (0..1000)
                .map(|i| ark_contract::WorkspaceChange {
                    path: format!("src/file_{i}.rs"),
                    operation: "write".into(),
                    turn_id: ark_common::TurnId::new(),
                    seq: i,
                })
                .collect(),
        };
        let prompt = review_prompt(&changes, Some("  src/api  "));
        assert!(prompt.is_ascii(), "prompt must stay ASCII-only");
        assert!(
            prompt.len() <= MAX_CHANGE_LIST_BYTES + 256,
            "prompt must stay bounded"
        );
        assert!(prompt.contains("Review focus: src/api"));
        assert!(prompt.contains("truncated"));
    }
}
