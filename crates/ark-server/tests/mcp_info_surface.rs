//! MCP discovery surface: the document answers honestly, and every tool it
//! advertises really exists on the router.
//!
//! The consistency probe is the load-bearing half (the acceptance section of
//! requirements/mcp-server-surface.md): the catalog and the route table live in different files, so the test
//! walks every advertised `method+path` against the real router. A route-level
//! miss is axum's bare 404 with an empty body; a handler-level miss is a
//! structured JSON 4xx. The probe must never see the former.

use std::sync::Arc;

use ark_common::Result;
use ark_config::{ConfigStore, RuntimeConfig};
use ark_obs::UsageRecorder;
use ark_ports::ToolRegistryPort;
use ark_server::{router, service::mcp_info, AppState, TurnEnd, TurnJob, TurnRunner};
use ark_store::{FileSessionStore, ScheduleFileStore, StoreConfig};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

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
impl TurnRunner for NoopRunner {
    async fn run_turn(&self, _job: TurnJob) -> Result<TurnEnd> {
        Ok(TurnEnd::Completed)
    }
}

fn state(data_dir: &std::path::Path) -> AppState {
    let sessions =
        Arc::new(FileSessionStore::open(StoreConfig::new(data_dir)).expect("sessions open"));
    let schedules = Arc::new(ark_server::SchedulerService::new(Arc::new(
        ScheduleFileStore::open(data_dir).expect("schedule store"),
    )));
    AppState::new(
        sessions,
        Arc::new(EmptyTools),
        Arc::new(ConfigStore::new(RuntimeConfig {
            data_dir: data_dir.to_path_buf(),
            ..RuntimeConfig::default()
        })),
        Arc::new(UsageRecorder::new()),
        Arc::new(NoopRunner),
        data_dir.to_path_buf(),
        "test",
    )
    .with_schedules(schedules)
}

fn app() -> axum::Router {
    let dir = tempfile::Builder::new()
        .prefix("ark-mcp-info-")
        .tempdir()
        .expect("temp dir");
    router(state(dir.path()))
}

async fn get_document(app: &axum::Router) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/mcp-info")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("document json")
}

#[tokio::test]
async fn discovery_document_answers_without_a_token() {
    let json = get_document(&app()).await;
    assert_eq!(json["server_name"], "arkd");
    assert_eq!(json["transport"], "http");
    assert_eq!(json["auth"]["required"], false);
    assert!(json["tools"].as_array().expect("tools").len() >= 40);
    assert!(json["tools"][0]["http"]["method"].is_string());
    assert!(json["tools"][0]["http"]["path"].is_string());
}

#[tokio::test]
async fn discovery_stays_open_when_a_token_is_configured() {
    let dir = tempfile::Builder::new()
        .prefix("ark-mcp-info-auth-")
        .tempdir()
        .expect("temp dir");
    let app = router(state(dir.path()).with_auth_token(Some("secret".to_string())));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/mcp-info")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(json["auth"]["required"], true);
    assert_eq!(json["auth"]["apply_as_header"], "authorization");
}

/// Every advertised tool must resolve to a real route. Placeholder replacement
/// uses format-valid dummy ids so the request reaches the handler, whose
/// structured 4xx proves the route exists.
#[tokio::test]
async fn every_advertised_tool_resolves_to_a_real_route() {
    let app = app();
    let catalog = mcp_info::server_info(false, "test").tools;
    assert!(catalog.len() >= 40, "catalog must not silently shrink");

    // Format-valid ids: parsing must succeed so the request reaches the handler
    // (structured 4xx), never the router's bare 404.
    let dummies: &[(&str, &str)] = &[
        ("{id}", "00000000-0000-0000-0000-000000000001"),
        ("{turn_id}", "00000000-0000-0000-0000-000000000002"),
        ("{child_id}", "00000000-0000-0000-0000-000000000003"),
    ];

    for tool in &catalog {
        let mut path = tool.http.path.to_string();
        for (placeholder, dummy) in dummies {
            path = path.replace(placeholder, dummy);
        }
        let request = Request::builder()
            .method(tool.http.method)
            .uri(&path)
            .header("content-type", "application/json")
            .body(Body::from(if tool.http.method == "GET" {
                Vec::new()
            } else {
                serde_json::to_vec(&serde_json::json!({})).expect("empty object")
            }))
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("response");
        let status = response.status();
        let empty = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body")
            .is_empty();
        assert!(
            !(status == StatusCode::NOT_FOUND && empty),
            "{} {} resolved to the router's bare 404: the catalog advertises a route the router does not have",
            tool.http.method,
            path
        );
    }
}

/// The wire shape the handler serves must match the assembled document.
#[tokio::test]
async fn served_document_matches_the_assembled_catalog() {
    let json = get_document(&app()).await;
    let assembled = mcp_info::server_info(false, "test");
    let names: Vec<&str> = json["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    let expected: Vec<&str> = assembled.tools.iter().map(|tool| tool.name).collect();
    assert_eq!(names, expected);
}
