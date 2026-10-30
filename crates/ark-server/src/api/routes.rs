//! The route table (design §7.9, appendix A).
//!
//! One place lists every path, so the surface is auditable and a future route
//! cannot appear without a deliberate edit here. Routes whose milestone has not
//! landed are registered explicitly and answer a structured `NotFound` from
//! [`crate::api::handlers::pending`] instead of 404-ing at the router level.

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, Method};
use axum::routing::{delete, get, patch, post};
use axum::Router;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::api::middleware::require_token;
use crate::state::AppState;

/// Build the full application router.
///
/// `/health` is mounted on its own router *before* the auth layer so it stays
/// unauthenticated, exactly as the design requires. The charset layer wraps the
/// whole surface so every JSON body declares `charset=utf-8` instead of leaving
/// the client to guess the platform default.
pub fn router(state: AppState) -> Router {
    let open = Router::new()
        .route("/health", get(crate::api::handlers::health::health))
        .with_state(state.clone());

    let protected = protected_routes().merge(attachment_routes(&state)).layer(
        axum::middleware::from_fn_with_state(state.clone(), require_token),
    );

    open.merge(protected)
        .layer(desktop_cors())
        .layer(axum::middleware::from_fn(
            crate::api::middleware::declare_utf8_charset,
        ))
        .with_state(state)
}

const ATTACHMENT_JSON_OVERHEAD_BYTES: usize = 64 * 1024;

fn attachment_routes(state: &AppState) -> Router<AppState> {
    let max_bytes = state
        .attachments
        .as_ref()
        .map(|store| store.max_bytes())
        .unwrap_or_default();
    let max_body_bytes = max_bytes
        .saturating_add(2)
        .saturating_div(3)
        .saturating_mul(4)
        .saturating_add(ATTACHMENT_JSON_OVERHEAD_BYTES as u64) as usize;

    let upload = Router::new()
        .route(
            "/v1/attachments",
            post(crate::api::handlers::attachments::upload),
        )
        .layer(DefaultBodyLimit::max(max_body_bytes));

    upload
        .route(
            "/v1/attachments/{id}",
            get(crate::api::handlers::attachments::get),
        )
        .route(
            "/v1/attachments/{id}/content",
            get(crate::api::handlers::attachments::content),
        )
        .route(
            "/v1/attachments/diagnostics",
            get(crate::api::handlers::attachments::diagnostics),
        )
}

/// Allow only the native shell origins to call the loopback runtime directly.
///
/// Vite development stays same-origin through its proxy. Tauri uses
/// `http://tauri.localhost` on Windows and `tauri://localhost` on macOS/Linux;
/// allowing neither would make the packaged shell unable to reach its own
/// loopback sidecar, while `*` would expose the local API to arbitrary pages.
fn desktop_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::list([
            HeaderValue::from_static("http://tauri.localhost"),
            HeaderValue::from_static("tauri://localhost"),
        ]))
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([header::ACCEPT, header::AUTHORIZATION, header::CONTENT_TYPE])
}

/// Every route that requires authentication.
fn protected_routes() -> Router<AppState> {
    Router::new()
        // ---- threads ----
        .route(
            "/v1/threads",
            get(crate::api::handlers::threads::list).post(crate::api::handlers::threads::create),
        )
        .route(
            "/v1/threads/{id}",
            get(crate::api::handlers::threads::get)
                .patch(crate::api::handlers::threads::update)
                .delete(crate::api::handlers::threads::remove),
        )
        // ---- turns ----
        .route(
            "/v1/threads/{id}/turns",
            post(crate::api::handlers::turns::start),
        )
        .route(
            "/v1/threads/{id}/turns/{turn_id}",
            get(crate::api::handlers::turns::get),
        )
        .route(
            "/v1/threads/{id}/turns/{turn_id}/steer",
            post(crate::api::handlers::turns::steer),
        )
        .route(
            "/v1/threads/{id}/turns/{turn_id}/interrupt",
            post(crate::api::handlers::turns::interrupt),
        )
        .route(
            "/v1/threads/{id}/compact",
            post(crate::api::handlers::turns::compact),
        )
        // ---- events (SSE) ----
        .route(
            "/v1/threads/{id}/events",
            get(crate::api::handlers::events::stream),
        )
        // ---- diagnostics ----
        .route("/v1/runtime/info", get(crate::api::handlers::runtime::info))
        .route(
            "/v1/runtime/tools",
            get(crate::api::handlers::runtime::tools),
        )
        .route(
            "/v1/runtime/capabilities",
            get(crate::api::handlers::runtime::capabilities),
        )
        .route("/v1/usage", get(crate::api::handlers::runtime::usage))
        .route(
            "/v1/debug/llm-rounds",
            get(crate::api::handlers::diagnostics::llm_rounds),
        )
        // ---- config (single entry point) ----
        .route(
            "/v1/runtime/config",
            get(crate::api::handlers::config_view::view).post(crate::api::handlers::config::apply),
        )
        // ---- lifecycle ----
        .route(
            "/v1/shutdown",
            post(crate::api::handlers::runtime::shutdown),
        )
        // ---- platform services (design §7.5.4) ----
        .route(
            "/v1/runtime/providers",
            get(crate::api::handlers::base::providers_list)
                .post(crate::api::handlers::base::providers_upsert),
        )
        .route(
            "/v1/runtime/providers/test",
            post(crate::api::handlers::base::providers_test),
        )
        .route(
            "/v1/runtime/providers/models",
            post(crate::api::handlers::base::providers_models),
        )
        .route(
            "/v1/runtime/providers/{id}",
            delete(crate::api::handlers::base::providers_delete),
        )
        .route(
            "/v1/runtime/providers/{id}/default",
            post(crate::api::handlers::base::providers_set_default),
        )
        .route("/v1/skills", get(crate::api::handlers::base::skills_list))
        .route(
            "/v1/workspace/status",
            get(crate::api::handlers::base::workspace_status),
        )
        .route(
            "/v1/workspace",
            get(crate::api::handlers::base::workspace_view),
        )
        .route(
            "/v1/workspace/switch",
            post(crate::api::handlers::base::workspace_switch),
        )
        .route(
            "/v1/workspace/diff",
            get(crate::api::handlers::base::workspace_diff),
        )
        .route(
            "/v1/workspace/validate",
            post(crate::api::handlers::base::workspace_validate),
        )
        .route(
            "/v1/mcp/servers",
            get(crate::api::handlers::base::mcp_list).post(crate::api::handlers::base::mcp_upsert),
        )
        .route(
            "/v1/mcp/servers/{id}",
            delete(crate::api::handlers::base::mcp_delete),
        )
        .route(
            "/v1/mcp/servers/{id}/test",
            post(crate::api::handlers::base::mcp_test),
        )
        // ---- thread planning, gates, workbench projections and child runs ----
        .route(
            "/v1/threads/{id}/fork",
            post(crate::api::handlers::threads::fork),
        )
        .route(
            "/v1/threads/{id}/plan",
            get(crate::api::handlers::workbench::plan),
        )
        .route(
            "/v1/threads/{id}/changes",
            get(crate::api::handlers::workbench::changes),
        )
        .route(
            "/v1/threads/{id}/files",
            get(crate::api::handlers::workbench::file),
        )
        .route(
            "/v1/threads/{id}/terminal",
            get(crate::api::handlers::workbench::terminal),
        )
        .route(
            "/v1/threads/{id}/goal",
            get(crate::api::handlers::workbench::plan)
                .post(crate::api::handlers::workbench::update_goal)
                .delete(crate::api::handlers::workbench::clear_goal),
        )
        .route(
            "/v1/threads/{id}/todos",
            get(crate::api::handlers::workbench::plan)
                .post(crate::api::handlers::workbench::update_todos)
                .delete(crate::api::handlers::workbench::clear_todos),
        )
        .route(
            "/v1/threads/{id}/skills",
            get(crate::api::handlers::base::thread_skills),
        )
        .route(
            "/v1/threads/{id}/usage",
            get(crate::api::handlers::diagnostics::thread_usage),
        )
        .route(
            "/v1/threads/{id}/skills/refresh",
            post(crate::api::handlers::base::thread_skills_refresh),
        )
        .route(
            "/v1/approvals/{id}",
            post(crate::api::handlers::gates::resolve_approval),
        )
        .route(
            "/v1/user-inputs/{id}",
            post(crate::api::handlers::gates::resolve_user_input),
        )
        .route(
            "/v1/user-input/{id}",
            post(crate::api::handlers::gates::resolve_user_input),
        )
        .route(
            "/v1/memory",
            get(crate::api::handlers::memory::list).post(crate::api::handlers::memory::create),
        )
        .route(
            "/v1/memory/{id}",
            patch(crate::api::handlers::memory::update)
                .delete(crate::api::handlers::memory::delete),
        )
        .route(
            "/v1/memory/diagnostics",
            get(crate::api::handlers::memory::diagnostics),
        )
        .route(
            "/v1/child-runs",
            get(crate::api::handlers::child_runs::list),
        )
        .route(
            "/v1/child-runs/{child_id}",
            get(crate::api::handlers::child_runs::get)
                .delete(crate::api::handlers::child_runs::remove),
        )
        .route(
            "/v1/child-runs/{child_id}/abort",
            post(crate::api::handlers::child_runs::abort),
        )
        .route(
            "/v1/child-runs/{child_id}/steer",
            post(crate::api::handlers::child_runs::steer),
        )
        .route(
            "/v1/child-runs/{child_id}/resume",
            post(crate::api::handlers::child_runs::resume),
        )
        .route(
            "/v1/child-runs/{child_id}/thread",
            get(crate::api::handlers::child_runs::thread),
        )
        .route(
            "/v1/child-runs/{child_id}/events",
            get(crate::api::handlers::child_runs::events),
        )
        .route(
            "/v1/sessions/{id}/resume-thread",
            post(crate::api::handlers::sessions::resume_thread),
        )
        .route(
            "/v1/threads/{id}/rewind",
            post(crate::api::handlers::rewind::rewind),
        )
        .route(
            "/v1/threads/{id}/review",
            post(crate::api::handlers::review::review),
        )
        .route(
            "/v1/threads/{id}/export",
            get(crate::api::handlers::export::export),
        )
        .route(
            "/v1/runtime/model-providers",
            post(crate::api::handlers::pending::model_providers),
        )
        .route(
            "/v1/subagents/config",
            get(crate::api::handlers::subagents::view)
                .post(crate::api::handlers::subagents::configure),
        )
        .route(
            "/v1/runtime/tuning",
            post(crate::api::handlers::pending::tuning),
        )
        .route(
            "/v1/connector-env",
            post(crate::api::handlers::pending::connector_env),
        )
        // `cleanup` remains a maintenance stub: reconciliation already removes
        // leftover tombstones on startup, so the route exists only so an old
        // client gets a structured answer instead of 404 at the router level.
        .route(
            "/v1/child-runs/{child_id}/cleanup",
            delete(crate::api::handlers::pending::child_runs),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{HeaderValue, Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn protected_routes_compile_into_a_router() {
        // Construction alone proves every path string is well formed for axum.
        let _ = protected_routes();
    }

    #[tokio::test]
    async fn desktop_cors_allows_only_the_documented_tauri_origin() {
        let app = Router::new()
            .route("/probe", get(|| async { StatusCode::NO_CONTENT }))
            .layer(desktop_cors());

        let desktop = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(header::ORIGIN, "http://tauri.localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            desktop.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("http://tauri.localhost"))
        );

        let foreign = app
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(header::ORIGIN, "https://untrusted.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(foreign
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
    }
}
