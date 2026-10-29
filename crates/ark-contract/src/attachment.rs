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
}
