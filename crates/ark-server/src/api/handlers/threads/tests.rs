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
    let _ = std::fs::remove_dir_all(data_dir);
}
