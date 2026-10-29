//! Thread routes (design §7.9): list, create, get, update, delete.
//!
//! The store is the system of record (design §7.4), so these handlers only
//! translate DTOs and publish the matching lifecycle event. No thread state is
//! cached here.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;

use ark_common::ThreadId;
use ark_contract::{
    ChildCompletionNotice, CreateThreadRequest, PendingApprovalView, PendingUserInputView,
    RuntimeEvent, ThreadDetail, ThreadListQuery, ThreadRuntimeSnapshot, ThreadSummary,
    UpdateThreadRequest, THREAD_PAGE_DEFAULT_LIMIT, THREAD_PAGE_MAX_LIMIT,
};
use ark_ports::ThreadMeta;
use std::collections::HashSet;

use crate::api::error::ApiError;
use crate::state::AppState;

/// Placeholder title when the client sends none.
pub const DEFAULT_TITLE: &str = "New thread";
const MAX_CHILD_COMPLETION_NOTICES: usize = 50;

/// `GET /v1/threads`.
///
/// Returns a bounded page of the store's deterministic ordering (active first,
/// then most recently updated, thread id as tiebreak). Absent query parameters
/// keep the legacy behaviour of listing every thread, so the response stays a
/// bare array; clients paginate with `limit` + `offset`, and an empty page
/// marks the end.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ThreadListQuery>,
) -> Result<Json<Vec<ThreadSummary>>, ApiError> {
    let limit = query
        .limit
        .unwrap_or(THREAD_PAGE_DEFAULT_LIMIT)
        .clamp(1, THREAD_PAGE_MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);
    let needle = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());

    let metas = state.store.list_threads().await?;
    // Deterministic filter-then-page: sorting here (instead of trusting index
    // order alone) keeps pagination stable across calls, because the tiebreak
    // is the immutable id.
    let mut rows: Vec<&ark_ports::ThreadMeta> = metas
        .iter()
        .filter(|meta| query.archived.is_none_or(|want| meta.archived == want))
        .filter(|meta| needle.is_none_or(|n| meta.title.to_lowercase().contains(&n.to_lowercase())))
        .collect();
    rows.sort_by(|a, b| {
        a.archived
            .cmp(&b.archived)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.thread_id.cmp(&b.thread_id))
    });

    let summaries = rows
        .iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|meta| summary_of(meta))
        .collect();
    Ok(Json(summaries))
}

/// `POST /v1/threads`.
pub async fn create(
    State(state): State<AppState>,
    body: Option<Json<CreateThreadRequest>>,
) -> Result<(StatusCode, Json<ThreadSummary>), ApiError> {
    let title = body
        .and_then(|Json(request)| request.title)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_TITLE.to_string());
    let meta = state.store.create_thread(&title).await?;
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
    Ok((StatusCode::CREATED, Json(summary)))
}

/// `GET /v1/threads/{id}`.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ThreadDetail>, ApiError> {
    let thread_id = parse_thread(&id)?;
    Ok(Json(load_detail(&state, thread_id).await?))
}

/// Build the full wire detail for one thread.
///
/// Shared by `GET /v1/threads/{id}` and `POST /v1/sessions/{id}/resume-thread`
/// so a resumed session observes exactly what a thread read observes.
pub(crate) async fn load_detail(
    state: &AppState,
    thread_id: ThreadId,
) -> Result<ThreadDetail, ApiError> {
    let meta = state.store.get_thread(thread_id).await?;
    let items = state.store.read_items(thread_id).await?;
    let child_completions = recent_child_completions(
        state
            .store
            .replay_child_completion_events(thread_id)
            .await?,
    );
    let child_ids = state
        .store
        .list_child_runs(thread_id)
        .await
        .map(|records| records.into_iter().map(|record| record.child_id).collect())
        .unwrap_or_default();
    let mut summary = summary_of(&meta);
    summary.last_seq = state.store.highest_seq(thread_id).await.ok();
    Ok(ThreadDetail {
        summary,
        items,
        child_ids,
        child_completions,
        runtime: meta.runtime,
        runtime_snapshot: project_runtime_snapshot(
            state.turns.active_for(thread_id),
            state.store.replay_interaction_events(thread_id).await?,
            state.gates.pending_approval_ids(),
            state.gates.pending_user_input_ids(),
        ),
    })
}

/// `PATCH /v1/threads/{id}`.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateThreadRequest>,
) -> Result<Json<ThreadSummary>, ApiError> {
    let thread_id = parse_thread(&id)?;
    let mut meta = state.store.get_thread(thread_id).await?;
    if let Some(title) = body.title {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                "title must not be empty",
            ));
        }
        meta = state.store.rename_thread(thread_id, trimmed).await?;
        let event = RuntimeEvent::ThreadUpdated {
            seq: 0,
            thread_id,
            timestamp: Utc::now(),
            title: meta.title.clone(),
        };
        state.store.append_event(thread_id, event).await?;
    }
    if let Some(archived) = body.archived {
        meta = state.store.set_archived(thread_id, archived).await?;
    }
    if let Some(runtime) = body.runtime {
        let merged = merge_runtime(meta.runtime, runtime, state.base.providers.as_deref())?;
        meta = state.store.set_thread_runtime(thread_id, merged).await?;
    }
    Ok(Json(summary_of(&meta)))
}

fn merge_runtime(
    current: ark_contract::ThreadRuntimeSettings,
    patch: ark_contract::ThreadRuntimeSettings,
    providers: Option<&dyn ark_ports::ProviderService>,
) -> Result<ark_contract::ThreadRuntimeSettings, ApiError> {
    let autonomy = patch.autonomy.or(current.autonomy);
    if let Some(value) = autonomy.as_deref() {
        if !matches!(value, "strict" | "workspace_write" | "unattended") {
            return Err(ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                "invalid autonomy",
            ));
        }
    }
    let provider_id = patch.provider_id.or(current.provider_id);
    let model = patch.model.or(current.model);
    if provider_id.is_some() || model.as_deref().is_some_and(|value| !value.is_empty()) {
        let providers = providers.ok_or_else(|| {
            ApiError::new(
                ark_common::ArkErrorCode::RuntimeUnavailable,
                "provider service is not available in this build",
            )
        })?;
        if providers.resolve_endpoint(provider_id.as_deref()).is_none() {
            let message = provider_id
                .as_deref()
                .map(|id| format!("thread provider is unavailable: {id}"))
                .unwrap_or_else(|| "thread model requires an available provider".to_string());
            return Err(ApiError::new(
                ark_common::ArkErrorCode::ValidationError,
                message,
            ));
        }
    }
    Ok(ark_contract::ThreadRuntimeSettings {
        workspace: patch.workspace.or(current.workspace),
        provider_id,
        model,
        autonomy,
    })
}

/// `DELETE /v1/threads/{id}`.
///
/// Refuses while a turn is still running. The store deletes the directory and
/// forgets the writer, but a live reactor keeps publishing: its next append
/// would recreate a directory with events but no `thread.json`, i.e. a thread
/// that cannot be listed, opened or cleaned up (design §7.4.1, P1-4).
///
/// Returns `409` so the caller can interrupt first and retry, matching the
/// existing "reject a second turn on a busy thread" policy.
pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let thread_id = parse_thread(&id)?;
    if let Some(active) = state.turns.active_for(thread_id) {
        return Err(ApiError::new(
            ark_common::ArkErrorCode::Conflict,
            format!(
                "thread has a running turn ({}) and cannot be deleted; interrupt it first",
                active.turn_id
            ),
        ));
    }
    state.store.delete_thread(thread_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/threads/{id}/fork`.
///
/// A fork always uses the current durable watermark. Forking from a selected
/// historical sequence needs an explicit history UI; accepting an arbitrary
/// number here would expose a partial feature with no way for the renderer to
/// explain which prefix it selected.
pub async fn fork(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<ThreadSummary>), ApiError> {
    let thread_id = parse_thread(&id)?;
    state.store.get_thread(thread_id).await?;
    let watermark = state.store.highest_seq(thread_id).await?;
    let forked = state.store.fork_thread(thread_id, watermark).await?;
    let mut summary = summary_of(&forked);
    summary.last_seq = Some(watermark);
    Ok((StatusCode::CREATED, Json(summary)))
}

/// Parse a thread id, reporting a validation error instead of a 500.
pub fn parse_thread(raw: &str) -> Result<ThreadId, ApiError> {
    raw.parse::<ThreadId>().map_err(|err| {
        ApiError::new(
            ark_common::ArkErrorCode::ValidationError,
            format!("`{raw}` is not a valid thread id: {err}"),
        )
    })
}

/// Map stored metadata onto the wire shape.
pub fn summary_of(meta: &ThreadMeta) -> ThreadSummary {
    ThreadSummary {
        thread_id: meta.thread_id,
        title: meta.title.clone(),
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        last_seq: None,
        archived: meta.archived,
    }
}

/// Keeps the newest distinct child completion cards within the bounded detail response.
fn recent_child_completions(events: Vec<RuntimeEvent>) -> Vec<ChildCompletionNotice> {
    let mut notification_ids = HashSet::new();
    let mut notices: Vec<_> = events
        .into_iter()
        .filter_map(|event| event.child_completion_notice())
        .rev()
        .filter(|notice| notification_ids.insert(notice.notification_id.clone()))
        .take(MAX_CHILD_COMPLETION_NOTICES)
        .collect();
    notices.reverse();
    notices
}

/// Builds the refresh snapshot from durable lifecycle events, then intersects
/// it with the live process-owned waiters. Durable history supplies display
/// fields; the waiter snapshot is the authorization boundary.
fn project_runtime_snapshot(
    active_turn: Option<ark_contract::TurnSummary>,
    events: Vec<RuntimeEvent>,
    live_approval_ids: impl IntoIterator<Item = impl AsRef<str>>,
    live_input_ids: impl IntoIterator<Item = impl AsRef<str>>,
) -> ThreadRuntimeSnapshot {
    let Some(active_turn) = active_turn else {
        return ThreadRuntimeSnapshot::default();
    };
    let live_approvals: HashSet<String> = live_approval_ids
        .into_iter()
        .map(|id| id.as_ref().to_string())
        .collect();
    let live_inputs: HashSet<String> = live_input_ids
        .into_iter()
        .map(|id| id.as_ref().to_string())
        .collect();
    let mut approvals = std::collections::HashMap::new();
    let mut inputs = std::collections::HashMap::new();
    for event in events {
        match event {
            RuntimeEvent::ApprovalRequested {
                approval_id,
                turn_id,
                tool,
                summary,
                timestamp,
                ..
            } if turn_id == active_turn.turn_id => {
                approvals.insert(
                    approval_id,
                    PendingApprovalView {
                        approval_id: String::new(),
                        turn_id,
                        tool,
                        summary,
                        requested_at: timestamp,
                    },
                );
            }
            RuntimeEvent::ApprovalResolved { approval_id, .. } => {
                approvals.remove(&approval_id);
            }
            RuntimeEvent::UserInputRequested {
                request_id,
                turn_id,
                prompt,
                timestamp,
                ..
            } if turn_id == active_turn.turn_id => {
                inputs.insert(
                    request_id,
                    PendingUserInputView {
                        request_id: String::new(),
                        turn_id,
                        prompt,
                        requested_at: timestamp,
                    },
                );
            }
            RuntimeEvent::UserInputResolved { request_id, .. } => {
                inputs.remove(&request_id);
            }
            _ => {}
        }
    }
    let mut pending_approvals: Vec<_> = approvals
        .into_iter()
        .filter_map(|(approval_id, mut view)| {
            live_approvals.contains(&approval_id).then(|| {
                view.approval_id = approval_id;
                view
            })
        })
        .collect();
    pending_approvals.sort_by_key(|view| view.requested_at);
    let mut pending_user_inputs: Vec<_> = inputs
        .into_iter()
        .filter_map(|(request_id, mut view)| {
            live_inputs.contains(&request_id).then(|| {
                view.request_id = request_id;
                view
            })
        })
        .collect();
    pending_user_inputs.sort_by_key(|view| view.requested_at);
    ThreadRuntimeSnapshot {
        active_turn: Some(active_turn),
        pending_approvals,
        pending_user_inputs,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use ark_common::{ChildId, TurnId};
    use ark_config::{ConfigStore, RuntimeConfig};
    use ark_obs::UsageRecorder;
    use ark_ports::{BaseServices, ModelEndpoint, ProviderService, SessionStore, ToolRegistryPort};
    use ark_store::{FileSessionStore, StoreConfig};
    use axum::extract::{Path, State};

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

    struct NoopRunner;

    #[async_trait::async_trait]
    impl crate::TurnRunner for NoopRunner {
        async fn run_turn(&self, _job: crate::TurnJob) -> ark_common::Result<crate::TurnEnd> {
            Ok(crate::TurnEnd::Completed)
        }
    }

    struct Providers;

    #[async_trait::async_trait]
    impl ProviderService for Providers {
        fn list(&self) -> ark_contract::ProviderListResponse {
            ark_contract::ProviderListResponse::default()
        }

        fn upsert(
            &self,
            _entry: ark_contract::ProviderEntry,
            _make_default: bool,
        ) -> ark_common::Result<ark_contract::ProviderListResponse> {
            unreachable!("not used by this test")
        }

        fn delete(&self, _id: &str) -> ark_common::Result<ark_contract::ProviderListResponse> {
            unreachable!("not used by this test")
        }

        fn set_default(&self, _id: &str) -> ark_common::Result<ark_contract::ProviderListResponse> {
            unreachable!("not used by this test")
        }

        async fn test(
            &self,
            _request: ark_contract::ProviderTestRequest,
        ) -> ark_contract::ProviderTestResponse {
            unreachable!("not used by this test")
        }

        async fn models(
            &self,
            _request: ark_contract::ProviderModelCatalogRequest,
        ) -> ark_common::Result<ark_contract::ProviderModelCatalogResponse> {
            unreachable!("not used by this test")
        }

        fn resolve_endpoint(&self, id: Option<&str>) -> Option<ModelEndpoint> {
            (id == Some("known")).then(|| ModelEndpoint {
                provider_id: "known".to_string(),
                model: "model".to_string(),
                base_url: "https://example.invalid".to_string(),
                api_key: "secret".to_string(),
                endpoint_format: "openai".to_string(),
                custom_endpoint: None,
                max_tokens: None,
                temperature: None,
            })
        }
    }

    fn state(data_dir: &std::path::Path) -> AppState {
        let sessions =
            Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("session store"));
        let store: Arc<dyn SessionStore> = sessions;
        AppState::new(
            store,
            Arc::new(EmptyTools),
            Arc::new(ConfigStore::new(RuntimeConfig::default())),
            Arc::new(UsageRecorder::new()),
            Arc::new(NoopRunner),
            data_dir.to_path_buf(),
            "test",
        )
        .with_base_services(BaseServices::new().with_providers(Arc::new(Providers)))
    }

    #[test]
    fn invalid_thread_ids_are_validation_errors() {
        let error = parse_thread("not-a-uuid").unwrap_err();
        assert_eq!(error.0.code, ark_common::ArkErrorCode::ValidationError);
    }

    #[test]
    fn valid_thread_ids_roundtrip() {
        let id = ThreadId::new();
        assert_eq!(parse_thread(&id.to_string()).unwrap(), id);
    }

    #[test]
    fn summary_mapping_preserves_title_and_archive_flag() {
        let meta = ThreadMeta {
            thread_id: ThreadId::new(),
            title: "hello".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            archived: true,
            runtime: ark_contract::ThreadRuntimeSettings::default(),
        };
        let summary = summary_of(&meta);
        assert_eq!(summary.title, "hello");
        assert!(summary.archived);
        assert_eq!(summary.last_seq, None);
    }

    #[test]
    fn thread_detail_keeps_only_the_latest_distinct_completion_cards() {
        let thread = ThreadId::new();
        let events = (1..=55)
            .map(|seq| RuntimeEvent::ChildCompleted {
                seq,
                thread_id: thread,
                timestamp: Utc::now(),
                notification_id: format!("child:{seq}"),
                child_id: ChildId::new(),
                label: "inspect runtime".to_string(),
                status: "completed".to_string(),
                summary: None,
                error: None,
                usage: None,
                tool_invocations: None,
            })
            .collect();

        let notices = recent_child_completions(events);

        assert_eq!(notices.len(), MAX_CHILD_COMPLETION_NOTICES);
        assert_eq!(notices.first().unwrap().seq, 6);
        assert_eq!(notices.last().unwrap().seq, 55);
    }

    #[test]
    fn interaction_projection_keeps_only_unresolved_live_gate_for_active_turn() {
        let thread = ThreadId::new();
        let active = TurnId::new();
        let finished = TurnId::new();
        let now = Utc::now();
        let runtime = project_runtime_snapshot(
            Some(ark_contract::TurnSummary {
                turn_id: active,
                thread_id: thread,
                state: "running".into(),
                started_at: now,
                ended_at: None,
                error: None,
            }),
            vec![
                RuntimeEvent::ApprovalRequested {
                    seq: 1,
                    thread_id: thread,
                    turn_id: active,
                    timestamp: now,
                    approval_id: "live".into(),
                    tool: "bash".into(),
                    summary: "run command".into(),
                },
                RuntimeEvent::ApprovalRequested {
                    seq: 2,
                    thread_id: thread,
                    turn_id: active,
                    timestamp: now,
                    approval_id: "resolved".into(),
                    tool: "write".into(),
                    summary: "update file".into(),
                },
                RuntimeEvent::ApprovalResolved {
                    seq: 3,
                    thread_id: thread,
                    turn_id: active,
                    timestamp: now,
                    approval_id: "resolved".into(),
                    approved: true,
                },
                RuntimeEvent::UserInputRequested {
                    seq: 4,
                    thread_id: thread,
                    turn_id: finished,
                    timestamp: now,
                    request_id: "old".into(),
                    prompt: "old prompt".into(),
                },
            ],
            ["live"],
            std::iter::empty::<&str>(),
        );

        assert_eq!(runtime.active_turn.unwrap().turn_id, active);
        assert_eq!(runtime.pending_approvals.len(), 1);
        assert_eq!(runtime.pending_approvals[0].approval_id, "live");
        assert!(runtime.pending_user_inputs.is_empty());
    }

    /// Seed threads with distinct titles and archive flags, oldest update first.
    async fn seed_threads(state: &AppState, count: usize) -> Vec<ThreadMeta> {
        let mut metas = Vec::new();
        for i in 0..count {
            let mut meta = state
                .store
                .create_thread(&format!("thread-{i:02}"))
                .await
                .expect("thread");
            if i % 2 == 1 {
                meta = state
                    .store
                    .set_archived(meta.thread_id, true)
                    .await
                    .expect("archive");
            }
            metas.push(meta);
        }
        metas
    }

    async fn list_response(state: AppState, query: ThreadListQuery) -> Vec<ThreadSummary> {
        list(State(state), Query(query)).await.expect("list").0
    }

    #[tokio::test]
    async fn list_without_query_stays_compatible() {
        let data_dir = std::env::temp_dir().join(format!("ark-thread-compat-{}", TurnId::new()));
        let state = state(&data_dir);
        seed_threads(&state, 5).await;

        let summaries = list_response(state.clone(), ThreadListQuery::default()).await;

        assert_eq!(summaries.len(), 5);
        // Deterministic order: active first, then most recently updated, so
        // the newest active thread (seeded last, even index) leads.
        assert_eq!(summaries[0].title, "thread-04");
        let _ = std::fs::remove_dir_all(PathBuf::from(&data_dir));
    }

    #[tokio::test]
    async fn limit_one_returns_exactly_one_page() {
        let data_dir = std::env::temp_dir().join(format!("ark-thread-limit-{}", TurnId::new()));
        let state = state(&data_dir);
        seed_threads(&state, 4).await;

        let page = list_response(
            state.clone(),
            ThreadListQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await;

        assert_eq!(page.len(), 1);
        assert_eq!(page[0].title, "thread-02");
        let _ = std::fs::remove_dir_all(PathBuf::from(&data_dir));
    }

    #[tokio::test]
    async fn offset_walks_pages_without_repetition_or_gap() {
        let data_dir = std::env::temp_dir().join(format!("ark-thread-offset-{}", TurnId::new()));
        let state = state(&data_dir);
        seed_threads(&state, 5).await;

        let first = list_response(
            state.clone(),
            ThreadListQuery {
                limit: Some(2),
                ..Default::default()
            },
        )
        .await;
        let second = list_response(
            state.clone(),
            ThreadListQuery {
                limit: Some(2),
                offset: Some(2),
                ..Default::default()
            },
        )
        .await;
        let third = list_response(
            state.clone(),
            ThreadListQuery {
                limit: Some(2),
                offset: Some(4),
                ..Default::default()
            },
        )
        .await;

        let titles: Vec<&str> = first
            .iter()
            .chain(&second)
            .chain(&third)
            .map(|s| s.title.as_str())
            .collect();
        assert_eq!(
            titles,
            vec![
                "thread-04",
                "thread-02",
                "thread-00",
                "thread-03",
                "thread-01"
            ]
        );
        let _ = std::fs::remove_dir_all(PathBuf::from(&data_dir));
    }

    #[tokio::test]
    async fn limit_clamps_zero_and_oversized_values() {
        let data_dir = std::env::temp_dir().join(format!("ark-thread-clamp-{}", TurnId::new()));
        let state = state(&data_dir);
        seed_threads(&state, 3).await;

        // limit=0 clamps to 1: still exactly one row, never an empty page.
        let zero = list_response(
            state.clone(),
            ThreadListQuery {
                limit: Some(0),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(zero.len(), 1);
        // Oversized values clamp to the contract maximum.
        let huge = list_response(
            state.clone(),
            ThreadListQuery {
                limit: Some(u32::MAX),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(huge.len(), 3);
        let _ = std::fs::remove_dir_all(PathBuf::from(&data_dir));
    }

    #[tokio::test]
    async fn archived_filter_composes_with_pagination() {
        let data_dir = std::env::temp_dir().join(format!("ark-thread-archived-{}", TurnId::new()));
        let state = state(&data_dir);
        seed_threads(&state, 4).await;

        let archived = list_response(
            state.clone(),
            ThreadListQuery {
                archived: Some(true),
                ..Default::default()
            },
        )
        .await;
        assert!(archived.iter().all(|s| s.archived));
        assert_eq!(archived.len(), 2);

        let active_paged = list_response(
            state.clone(),
            ThreadListQuery {
                archived: Some(false),
                limit: Some(1),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(active_paged.len(), 1);
        assert!(!active_paged[0].archived);
        let _ = std::fs::remove_dir_all(PathBuf::from(&data_dir));
    }

    #[tokio::test]
    async fn title_search_composes_with_pagination() {
        let data_dir = std::env::temp_dir().join(format!("ark-thread-search-{}", TurnId::new()));
        let state = state(&data_dir);
        for title in ["rust review", "rust design", "other"] {
            state.store.create_thread(title).await.expect("thread");
        }

        let hits = list_response(
            state.clone(),
            ThreadListQuery {
                q: Some("RUST".into()),
                limit: Some(1),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(hits.len(), 1);
        assert!(hits[0].title.starts_with("rust"));
        let _ = std::fs::remove_dir_all(PathBuf::from(&data_dir));
    }

    #[tokio::test]
    async fn unknown_provider_patch_is_rejected_without_changing_runtime() {
        let data_dir = std::env::temp_dir().join(format!("ark-thread-provider-{}", TurnId::new()));
        let state = state(&data_dir);
        let thread = state.store.create_thread("provider").await.expect("thread");

        let error = update(
            State(state.clone()),
            Path(thread.thread_id.to_string()),
            Json(UpdateThreadRequest {
                title: None,
                archived: None,
                runtime: Some(ark_contract::ThreadRuntimeSettings {
                    provider_id: Some("does-not-exist".to_string()),
                    ..Default::default()
                }),
            }),
        )
        .await
        .expect_err("unknown provider must be rejected");

        assert_eq!(error.0.code, ark_common::ArkErrorCode::ValidationError);
        assert_eq!(error.0.code.http_status(), 400);
        assert_eq!(
            state
                .store
                .get_thread(thread.thread_id)
                .await
                .expect("thread")
                .runtime
                .provider_id,
            None
        );
        let _ = std::fs::remove_dir_all(PathBuf::from(data_dir));
    }
}
