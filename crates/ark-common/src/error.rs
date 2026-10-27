use serde::{Deserialize, Serialize};

/// Stable, machine-readable error taxonomy (design §6.2).
///
/// The renderer must never hand-maintain a second union of these values, so the
/// enum is the single source of truth for both the wire payload and the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
#[serde(rename_all = "snake_case")]
pub enum ArkErrorCode {
    ValidationError,
    NotFound,
    Unauthorized,
    Conflict,
    ModelRequestFailed,
    ToolExecutionFailed,
    RuntimeUnavailable,
    StoreFailure,
    NotImplemented,
    InternalError,
}

impl ArkErrorCode {
    /// HTTP status used by `ark-server`. Kept next to the enum so a new code
    /// cannot be added without deciding its transport status.
    pub fn http_status(self) -> u16 {
        match self {
            ArkErrorCode::ValidationError => 400,
            ArkErrorCode::Unauthorized => 401,
            ArkErrorCode::NotFound => 404,
            ArkErrorCode::Conflict => 409,
            ArkErrorCode::ModelRequestFailed => 502,
            ArkErrorCode::ToolExecutionFailed => 500,
            ArkErrorCode::RuntimeUnavailable => 503,
            ArkErrorCode::StoreFailure => 500,
            ArkErrorCode::NotImplemented => 501,
            ArkErrorCode::InternalError => 500,
        }
    }
}

/// Wire shape of an error. This is what SSE `error` events carry and what the
/// HTTP error body contains.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(optional_fields)]
pub struct ArkErrorPayload {
    pub code: ArkErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ArkError {
    pub code: ArkErrorCode,
    pub message: String,
    pub detail: Option<String>,
}

impl ArkError {
    pub fn new(code: ArkErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ArkErrorCode::ValidationError, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ArkErrorCode::NotFound, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ArkErrorCode::Conflict, message)
    }

    pub fn store(message: impl Into<String>) -> Self {
        Self::new(ArkErrorCode::StoreFailure, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ArkErrorCode::InternalError, message)
    }

    pub fn model(message: impl Into<String>) -> Self {
        Self::new(ArkErrorCode::ModelRequestFailed, message)
    }

    pub fn payload(&self) -> ArkErrorPayload {
        ArkErrorPayload {
            code: self.code,
            message: self.message.clone(),
            detail: self.detail.clone(),
        }
    }
}

impl From<std::io::Error> for ArkError {
    fn from(err: std::io::Error) -> Self {
        Self::new(ArkErrorCode::StoreFailure, err.to_string())
    }
}

impl From<serde_json::Error> for ArkError {
    fn from(err: serde_json::Error) -> Self {
        Self::new(ArkErrorCode::ValidationError, err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ArkError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping_is_total() {
        assert_eq!(ArkErrorCode::NotFound.http_status(), 404);
        assert_eq!(ArkErrorCode::ValidationError.http_status(), 400);
    }

    #[test]
    fn payload_roundtrip() {
        let err = ArkError::not_found("thread missing").with_detail("thr_1");
        let json = serde_json::to_string(&err.payload()).unwrap();
        assert!(json.contains("\"code\":\"not_found\""));
        assert!(json.contains("thr_1"));
    }
}
