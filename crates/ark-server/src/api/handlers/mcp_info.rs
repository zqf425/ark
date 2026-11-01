//! `GET /v1/mcp-info`: the self-describing MCP discovery document
//! (requirements/mcp-server-surface.md).
//!
//! Mounted outside the auth layer on purpose: a caller must read this document
//! to learn *how* to authenticate, so discovery cannot require the token it
//! explains. The document carries no sensitive data: tool names mirror the
//! public route table, and `auth` only states whether a token is configured.

use axum::extract::State;
use axum::Json;

use crate::service::mcp_info;
use crate::state::AppState;

pub async fn info(State(state): State<AppState>) -> Json<serde_json::Value> {
    let document = mcp_info::server_info(state.auth_token.is_some(), &state.version);
    Json(document.to_json())
}
