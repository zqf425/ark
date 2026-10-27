//! Auth middleware (design §7.9).
//!
//! Authentication is one layer, applied to the protected router. Routes never
//! contain an `if !authorize(..)` branch, which is the 46-fold duplication the
//! design asks us to remove.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::api::error::ApiError;
use crate::state::AppState;

/// Header carrying the bearer token.
pub const AUTH_HEADER: &str = "authorization";

/// Reject requests that do not carry the configured bearer token.
///
/// When `auth_token` is `None` the runtime is local-only and every request is
/// allowed; that is an explicit configuration, not a silent default.
pub async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let Some(expected) = state.auth_token.as_deref() else {
        return Ok(next.run(request).await);
    };
    let presented = request
        .headers()
        .get(AUTH_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    match presented {
        Some(token) if constant_time_eq(token, expected) => Ok(next.run(request).await),
        Some(_) => Err(ApiError::new(
            ark_common::ArkErrorCode::Unauthorized,
            "invalid bearer token",
        )),
        None => Err(ApiError::new(
            ark_common::ArkErrorCode::Unauthorized,
            "missing `authorization: Bearer <token>` header",
        )),
    }
}

/// Compare two secrets without an early-exit length leak.
fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Declare the charset on JSON responses.
///
/// `axum::Json` emits `content-type: application/json` with no charset. Every
/// byte the runtime writes is UTF-8 (design §6.1), but a client that has to
/// guess may decode the body as the platform default - on a Chinese Windows host
/// that is GBK, so CJK titles render as mojibake even though the stored bytes are
/// correct. Stating `charset=utf-8` removes the guess.
pub async fn declare_utf8_charset(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    let is_json = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let lower = value.to_ascii_lowercase();
            lower.starts_with("application/json") && !lower.contains("charset=")
        });
    if is_json {
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json; charset=utf-8"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_tokens_match() {
        assert!(constant_time_eq("abc", "abc"));
    }

    #[test]
    fn different_tokens_do_not_match() {
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "abc"));
    }
}
