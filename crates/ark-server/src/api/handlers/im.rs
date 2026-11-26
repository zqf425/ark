//! HTTP surface for IM channels (requirements/im-channels.md §HTTP API).
//!
//! Handlers stay thin: parse, call `ark-im` for the domain decisions
//! (normalization, merge semantics, capacity), call the store, notify the
//! service to reconcile. No rule lives here that the orchestration would have
//! to re-derive — a second copy is how the settings page and the running
//! channels would start disagreeing.
//!
//! The master switch (`config.im.enabled`) gates *connections*, not editing:
//! a user who turned IM off can still list, create and fix channels.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use ark_common::ArkError;
use ark_contract::{
    CreateImChannelRequest, ImBindingListResponse, ImChannelId, ImChannelListItem,
    ImChannelListResponse, ImChannelView, ImVerifyResponse, ImWeixinLoginStartResponse,
    ImWeixinLoginWaitResponse, UpdateImChannelRequest,
};
use ark_im::{apply_update, ensure_channel_capacity, prepare_create};
use ark_ports::ImChannelRecord;

use crate::api::error::ApiError;
use crate::state::AppState;

/// `GET /v1/im/channels` — every channel with its live runtime state.
pub async fn list(State(state): State<AppState>) -> Result<Json<ImChannelListResponse>, ApiError> {
    let service = state.im_service()?;
    let records = service.store().list_channels()?;
    let channels = records
        .into_iter()
        .map(|record| ImChannelListItem {
            channel: record.channel.to_view(),
            runtime: service.channel_runtime(&record.channel.id),
        })
        .collect();
    Ok(Json(ImChannelListResponse { channels }))
}

/// `POST /v1/im/channels` — create one channel (starts nothing directly; the
/// service's sync loop picks it up within moments).
pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateImChannelRequest>,
) -> Result<(StatusCode, Json<ImChannelView>), ApiError> {
    let service = state.im_service()?;
    let existing = service.store().list_channels()?.len();
    ensure_channel_capacity(existing)?;
    let channel = prepare_create(request)?;
    let stored = service.store().insert_channel(ImChannelRecord {
        channel,
        cursor: None,
    })?;
    service.request_sync();
    Ok((StatusCode::CREATED, Json(stored.channel.to_view())))
}

/// `PATCH /v1/im/channels/{id}` — merge an update onto a stored channel.
///
/// The store retains its current cursor under the write lock: it is server
/// bookkeeping, no client may write it, and a stale PATCH cannot roll it back.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateImChannelRequest>,
) -> Result<Json<ImChannelView>, ApiError> {
    let service = state.im_service()?;
    let id = ImChannelId(id);
    let record = service.store().get_channel(&id)?;
    let channel = apply_update(&record.channel, request)?;
    let stored = service.store().replace_channel(ImChannelRecord {
        channel,
        cursor: None,
    })?;
    service.request_sync();
    Ok(Json(stored.channel.to_view()))
}

/// `DELETE /v1/im/channels/{id}` — stop the connection, then remove the
/// channel; the store cascades its bindings in the same critical section.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let service = state.im_service()?;
    let id = ImChannelId(id);
    service.stop_channel(&id);
    service.store().delete_channel(&id)?;
    service.request_sync();
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/im/channels/{id}/verify` — validate the stored credentials
/// against the platform and return its identity.
pub async fn verify(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ImVerifyResponse>, ApiError> {
    let service = state.im_service()?;
    let id = ImChannelId(id);
    let record = service.store().get_channel(&id)?;
    let credentials = record
        .channel
        .credentials
        .as_ref()
        .ok_or_else(|| ArkError::validation("channel has no credentials"))?;
    let transport = ark_im::build_transport(
        service.client().clone(),
        record.channel.kind,
        credentials,
        None,
    )?;
    let identity = transport.verify().await?;
    Ok(Json(ImVerifyResponse { identity }))
}

#[derive(Deserialize)]
pub struct BindingQuery {
    #[serde(default)]
    pub channel_id: Option<String>,
}

/// `GET /v1/im/bindings` — the read-only chat→thread mapping table.
pub async fn bindings(
    State(state): State<AppState>,
    Query(query): Query<BindingQuery>,
) -> Result<Json<ImBindingListResponse>, ApiError> {
    let service = state.im_service()?;
    let channel_id = query.channel_id.map(ImChannelId);
    let list = service.store().list_bindings(channel_id.as_ref())?;
    Ok(Json(ImBindingListResponse { bindings: list }))
}

/// `POST /v1/im/weixin/login/start` — fetch a QR code session.
pub async fn weixin_login_start(
    State(state): State<AppState>,
) -> Result<Json<ImWeixinLoginStartResponse>, ApiError> {
    let service = state.im_service()?;
    Ok(Json(service.weixin_login_start().await?))
}

#[derive(Deserialize)]
pub struct LoginWaitQuery {
    pub session_id: String,
}

/// `GET /v1/im/weixin/login/wait` — poll once; a confirmed login creates the
/// (disabled) channel server-side.
pub async fn weixin_login_wait(
    State(state): State<AppState>,
    Query(query): Query<LoginWaitQuery>,
) -> Result<Json<ImWeixinLoginWaitResponse>, ApiError> {
    let service = state.im_service()?;
    Ok(Json(service.weixin_login_wait(&query.session_id).await?))
}
