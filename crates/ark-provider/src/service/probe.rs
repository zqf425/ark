//! Connectivity probe for one provider entry.
//!
//! [`probe`] sends a deliberately tiny request through
//! `ark_llm::HttpModelClient` — the same client the agent loop uses — so a
//! successful probe proves the same wire family, headers and URL resolution the
//! real traffic will use. `max_tokens` is small and retries are disabled: a probe
//! reports the first observed failure instead of burning time on backoff.
//!
//! Every message leaving this module is scrubbed: the request URL, the provider
//! message and the response body can all echo a credential.

use std::time::{Duration, Instant};

use ark_common::text::truncate_utf8;
use ark_common::{ArkError, ArkErrorCode};
use ark_contract::ProviderTestResponse;
use ark_llm::common::retry::RetryPolicy;
use ark_llm::HttpModelClient;
use ark_ports::{ModelChunk, ModelClient, ModelMessage, ModelRequest};
use tokio::sync::mpsc;

use crate::config::{
    PROBE_BODY_LIMIT, PROBE_MAX_TOKENS, PROBE_OK_STATUS, PROBE_PROMPT, PROBE_TIMEOUT,
};
use crate::domain::{redact_url, scrub_secret};

/// Channel depth for the drain task; the probe discards every chunk.
const SINK_CAPACITY: usize = 16;

/// Extra wait on top of the client timeout so a trickling response cannot hang
/// the settings dialog forever.
const PROBE_GRACE: Duration = Duration::from_secs(2);

/// One tiny round trip.
pub async fn probe(config: ark_llm::ProviderConfig) -> ProviderTestResponse {
    let secret = config.api_key.clone();
    let client = HttpModelClient::with_config(config)
        .with_retry(RetryPolicy::none())
        .with_timeout(PROBE_TIMEOUT);
    let request_url = redact_url(&client.request_url());
    let label = format!("{} {}", client.provider_id(), client.model());

    let request = ModelRequest {
        messages: vec![ModelMessage::user(PROBE_PROMPT)],
        max_tokens: Some(PROBE_MAX_TOKENS),
        ..Default::default()
    };
    let (sink, mut drain) = mpsc::channel::<ModelChunk>(SINK_CAPACITY);
    // The probe ignores content, but an undrained channel would stall the
    // client once the buffer fills.
    tokio::spawn(async move { while drain.recv().await.is_some() {} });

    let started = Instant::now();
    let outcome =
        tokio::time::timeout(PROBE_TIMEOUT + PROBE_GRACE, client.stream(request, sink)).await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match outcome {
        Ok(Ok(_)) => ProviderTestResponse {
            ok: true,
            status: Some(PROBE_OK_STATUS),
            latency_ms,
            message: format!("connected to {label} in {latency_ms} ms"),
            request_url: Some(request_url),
        },
        Ok(Err(error)) => failure(&error, latency_ms, request_url, &secret),
        Err(_) => ProviderTestResponse {
            ok: false,
            status: None,
            latency_ms,
            message: format!(
                "request to {request_url} timed out after {}s; check the base URL and network",
                (PROBE_TIMEOUT + PROBE_GRACE).as_secs()
            ),
            request_url: Some(request_url),
        },
    }
}

/// Turn a client error into a probe response, classifying the failure so the
/// dialog can say "timed out" instead of showing a raw transport string.
fn failure(
    error: &ArkError,
    latency_ms: u64,
    request_url: String,
    secret: &str,
) -> ProviderTestResponse {
    let message = scrub_secret(&error.message, secret);
    let detail = error
        .detail
        .as_deref()
        .map(|body| scrub_secret(&bounded_body(body), secret))
        .unwrap_or_default();

    if let Some(status) = http_status(&message) {
        let body = if detail.is_empty() {
            String::new()
        } else {
            format!(" {detail}")
        };
        return ProviderTestResponse {
            ok: false,
            status: Some(status),
            latency_ms,
            message: format!(
                "HTTP {status}:{}",
                if body.is_empty() {
                    " no response body".into()
                } else {
                    body
                }
            ),
            request_url: Some(request_url),
        };
    }

    let lowered = message.to_ascii_lowercase();
    let hint = if lowered.contains("timed out") || lowered.contains("timeout") {
        format!(
            "connection to {request_url} timed out after {}s; check the base URL and network",
            PROBE_TIMEOUT.as_secs()
        )
    } else if lowered.contains("connect") || lowered.contains("sending request") {
        format!("cannot connect to {request_url}; check the base URL, proxy and TLS settings")
    } else {
        format!("request to {request_url} failed")
    };
    let mut text = hint;
    if !detail.is_empty() {
        text.push_str(": ");
        text.push_str(&detail);
    } else if !message.is_empty() {
        text.push_str(": ");
        text.push_str(truncate_utf8(&message, PROBE_BODY_LIMIT));
    }
    ProviderTestResponse {
        ok: false,
        status: None,
        latency_ms,
        message: text,
        request_url: Some(request_url),
    }
}

/// Response body kept from a failed request, cut to the displayed budget.
fn bounded_body(body: &str) -> String {
    truncate_utf8(body.trim(), PROBE_BODY_LIMIT).to_string()
}

/// Extract `with HTTP <code>` from a model-client error message.
///
/// `ark_llm::HttpModelClient` reports a non-2xx status inside its error message
/// rather than as a typed field, so the probe reads it back out. Only the trailer
/// after the *last* marker is considered, and digits must follow immediately.
pub fn http_status(message: &str) -> Option<u16> {
    let tail = message.rsplit("with HTTP ").next()?;
    if std::ptr::eq(tail, message) {
        return None;
    }
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits
        .parse::<u16>()
        .ok()
        .filter(|code| (100..600).contains(code))
}

/// Build the "cannot probe" response used when a request never reaches the wire
/// (missing key, unknown provider, invalid draft).
pub fn unusable(message: impl Into<String>, request_url: Option<String>) -> ProviderTestResponse {
    ProviderTestResponse {
        ok: false,
        status: None,
        latency_ms: 0,
        message: message.into(),
        request_url,
    }
}

/// True when an error code means the transport never produced a status line.
pub fn is_transport_failure(code: ArkErrorCode) -> bool {
    matches!(
        code,
        ArkErrorCode::ModelRequestFailed | ArkErrorCode::RuntimeUnavailable
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_is_read_back_from_the_client_message() {
        let message =
            "model request failed for p m at https://x.local/v1/chat/completions with HTTP 404";
        assert_eq!(http_status(message), Some(404));
        let message = "model request failed for p m at https://x.local/v1 with HTTP 429";
        assert_eq!(http_status(message), Some(429));
    }

    #[test]
    fn a_message_without_a_status_yields_none() {
        assert_eq!(http_status("model stream broke: connection reset"), None);
        assert_eq!(http_status("with HTTP abc"), None);
        assert_eq!(http_status("with HTTP 999"), None);
    }

    #[test]
    fn a_timeout_is_named_as_a_timeout_not_a_generic_failure() {
        let error = ArkError::model(
            "model request failed for p m at https://x.local/v1 (x): operation timed out",
        );
        let response = failure(&error, 10_000, "https://x.local/v1".into(), "sk-secret");
        assert!(!response.ok);
        assert_eq!(response.status, None);
        assert!(
            response.message.contains("timed out"),
            "{}",
            response.message
        );
    }

    #[test]
    fn a_connect_error_points_at_the_base_url() {
        let error =
            ArkError::model("model request failed for p m at https://x.local/v1: error sending request: connection refused");
        let response = failure(&error, 5, "https://x.local/v1".into(), "");
        assert!(
            response.message.contains("cannot connect"),
            "{}",
            response.message
        );
    }

    #[test]
    fn a_non_2xx_body_is_truncated_and_scrubbed() {
        let body = format!("{} sk-secret-tail", "x".repeat(900));
        let error =
            ArkError::model("model request failed for p m at https://x.local/v1 with HTTP 401")
                .with_detail(body);
        let response = failure(&error, 3, "https://x.local/v1".into(), "sk-secret-tail");
        assert_eq!(response.status, Some(401));
        assert!(
            response.message.starts_with("HTTP 401:"),
            "{}",
            response.message
        );
        assert!(!response.message.contains("sk-secret-tail"));
        // The body part stays inside the display budget plus the prefix.
        assert!(
            response.message.len() <= PROBE_BODY_LIMIT + 64,
            "{}",
            response.message.len()
        );
    }

    #[test]
    fn an_error_without_a_body_still_explains_itself() {
        let error =
            ArkError::model("model request failed for p m at https://x.local/v1 with HTTP 500");
        let response = failure(&error, 3, "https://x.local/v1".into(), "");
        assert!(
            response.message.contains("no response body"),
            "{}",
            response.message
        );
    }

    #[test]
    fn transport_failures_are_classified() {
        assert!(is_transport_failure(ArkErrorCode::ModelRequestFailed));
        assert!(!is_transport_failure(ArkErrorCode::ValidationError));
    }
}
