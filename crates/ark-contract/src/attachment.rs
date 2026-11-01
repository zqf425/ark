//! Wire types for persisted user attachments.

use ark_common::AttachmentId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Immutable metadata returned for one uploaded attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct AttachmentEntry {
    pub id: AttachmentId,
    pub original_filename: String,
    #[ts(as = "f64")]
    pub byte_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_text: Option<String>,
    /// Lowercase hex SHA-256 of the stored bytes, computed at upload.
    ///
    /// Integrity and future deduplication only: storage stays UUID-addressed so
    /// no existing path has to migrate. `#[serde(default)]` keeps metadata
    /// written before this field existed readable (empty = not recorded).
    #[serde(default)]
    pub sha256: String,
}

/// JSON upload body for `POST /v1/attachments`.
///
/// `content` is standard Base64 so binary bytes never enter metadata storage.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct AttachmentUploadRequest {
    pub original_filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_text: Option<String>,
    pub content: String,
}

/// Successful upload response.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct AttachmentUploadResponse {
    pub attachment: AttachmentEntry,
}

/// Storage usage returned by `GET /v1/attachments/diagnostics`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct AttachmentDiagnostics {
    pub storage_path: String,
    #[ts(as = "f64")]
    pub entry_count: u64,
    #[ts(as = "f64")]
    pub total_bytes: u64,
    /// Surviving attachments that no thread associated and that are older than
    /// the orphan threshold. Reported only; automatic collection is undecided.
    #[ts(as = "f64")]
    pub orphan_count: u64,
}

/// JSON body for `POST /v1/threads/{thread_id}/attachments`.
///
/// Association is the authorization edge: a turn may only reference attachments
/// that were associated with its own thread.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct AttachmentAssociationRequest {
    pub attachment_id: AttachmentId,
}

/// Successful association response. Repeating the call returns the same entry
/// without appending a second durable event.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct AttachmentAssociationResponse {
    pub attachment: AttachmentEntry,
}
