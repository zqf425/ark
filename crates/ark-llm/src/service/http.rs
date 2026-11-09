//! The real HTTP model client (design §7.2).
//!
//! Split of responsibilities: [`crate::provider::route::Family`] owns the wire
//! format (URL path, body shape, SSE frame meaning), [`crate::common::retry`]
//! owns the backoff schedule, and this module owns transport only — send, read
//! bytes, hand them to the SSE parser, forward chunks as they arrive.
//!
//! Nothing here buffers the whole reply: each `data:` frame is parsed and
//! forwarded before the next one is read, which is the memory-peak fix the old
//! runtime needed.
//!
//! There is deliberately no wall-clock deadline on a whole turn: reasoning
//! models legitimately stream for many minutes. What is bounded is each
//! *phase* — waiting for response headers, and waiting for the next body
//! frame — so a dead connection cannot hang the runtime while a live long
//! stream survives. See [`DEFAULT_PHASE_TIMEOUT`].

use std::time::Duration;

use ark_common::{ArkError, ArkErrorCode, Result};
use ark_ports::{ModelChunk, ModelClient, ModelRequest, ModelResponse};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::common::retry::{jittered, parse_retry_after, FailureKind, RetryPolicy, RetryState};
use crate::common::sse::SseParser;
use crate::common::stream::StreamAccumulator;
use crate::config::endpoint::ProviderConfig;
use crate::provider::route::Family;
use crate::provider::StreamParser;

/// Maximum time to wait for one phase of a model request: response headers,
/// or a single body frame. Not a total deadline for the whole turn.
///
/// reqwest's request-level `timeout()` is a *total* deadline that also covers
/// reading the body, so using it here cut long SSE streams off at exactly 120s
/// and reported the truncation as the misleading "error decoding response
/// body". Bounding the silence between reads instead lets a live stream run as
/// long as it needs while a stalled one still fails promptly.
const DEFAULT_PHASE_TIMEOUT: Duration = Duration::from_secs(120);

/// HTTP-backed [`ModelClient`].
pub struct HttpModelClient {
    config: ProviderConfig,
    client: reqwest::Client,
    retry: RetryPolicy,
    /// Bound on each request phase (headers / one body frame), not on the
    /// whole turn. See [`DEFAULT_PHASE_TIMEOUT`].
    timeout: Duration,
}

impl std::fmt::Debug for HttpModelClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpModelClient")
            .field("provider_id", &self.config.provider_id)
            .field("model", &self.config.model)
            .field("endpoint_format", &self.config.endpoint_format)
            .field("url", &self.config.request_url())
            .finish_non_exhaustive()
    }
}

impl HttpModelClient {
    /// Build a client around an injected `reqwest::Client`.
    ///
    /// The client is injected rather than constructed here so there is exactly
    /// one proxy/TLS configuration in the process (design §7.2, fixes C-6).
    pub fn new(config: ProviderConfig, client: reqwest::Client) -> Self {
        Self {
            config,
            client,
            retry: RetryPolicy::default(),
            timeout: DEFAULT_PHASE_TIMEOUT,
        }
    }

    /// Build a client with a default `reqwest::Client`.
    ///
    /// No client-level timeout: `ClientBuilder::timeout` is a total deadline
    /// covering the body read too, which would truncate long SSE streams. The
    /// phase bounds live in `send_once`/`pump` instead, so they behave the same
    /// for clients injected through [`HttpModelClient::new`] regardless of who
    /// built the transport.
    pub fn with_config(config: ProviderConfig) -> Self {
        let client = reqwest::Client::builder().build().unwrap_or_default();
        Self::new(config, client)
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Narrow the per-phase bound (response headers / one body frame).
    ///
    /// `probe` uses a short value so the settings dialog cannot hang, and
    /// tests use it to make timeout behaviour deterministic. This is **not**
    /// a total deadline for a turn.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    /// The URL this client would call. Exposed so a 404 can be explained by
    /// pointing at provider configuration instead of guessing.
    pub fn request_url(&self) -> String {
        self.config.request_url()
    }

    /// Serialise one request for this client's family.
    fn body_for(&self, family: &Family, request: &ModelRequest) -> Result<serde_json::Value> {
        family.body(&self.config, request)
    }
}

#[async_trait::async_trait]
impl ModelClient for HttpModelClient {
    fn provider_id(&self) -> &str {
        &self.config.provider_id
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    async fn stream(
        &self,
        request: ModelRequest,
        sink: mpsc::Sender<ModelChunk>,
    ) -> Result<ModelResponse> {
        let mut state = RetryState::default();
        loop {
            match self.attempt(&request, &sink).await {
                Ok(response) => return Ok(response),
                Err(failure) => {
                    let kind = failure.kind;
                    match self.retry.next(state, kind) {
                        Some(next) => {
                            // A retry after the sink already received deltas
                            // would duplicate text, so only retry before any
                            // content was published.
                            if failure.published {
                                return Err(failure.error);
                            }
                            state = next;
                            // A provider-supplied Retry-After overrides the
                            // backoff schedule (already capped by the parser);
                            // jitter desynchronises concurrent turns hitting
                            // the same rate-limit window. A zero value counts
                            // as "not provided" — the same rule as smart-ark's
                            // `computeRetryDelay`, which only adopts
                            // `retryAfterMs` when it is > 0 — so
                            // `Retry-After: 0` cannot bypass the rate-limit
                            // floor and hammer the provider.
                            let base = failure
                                .retry_after
                                .filter(|delay| !delay.is_zero())
                                .unwrap_or(next.next_delay);
                            let delay = jittered(base, salt_for(state.failures));
                            tracing::warn!(
                                provider = %self.config.provider_id,
                                model = %self.config.model,
                                kind = ?kind,
                                attempt = state.failures,
                                max_attempts = self.retry.max_attempts,
                                delay_ms = delay.as_millis() as u64,
                                "transient model request failure; retrying"
                            );
                            tokio::time::sleep(delay).await;
                        }
                        None => return Err(failure.error),
                    }
                }
            }
        }
    }
}

/// Salt for [`jittered`]: wall-clock nanos mixed with the attempt count, so
/// concurrent turns on the same rate-limit window do not retry in lockstep.
fn salt_for(failures: u32) -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64 ^ u64::from(failures).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .unwrap_or(u64::from(failures))
}

/// A failed attempt plus what it already published.
struct AttemptError {
    error: ArkError,
    kind: FailureKind,
    /// True once at least one chunk reached the sink.
    published: bool,
    /// Provider-advised wait from a `Retry-After` header, already capped.
    retry_after: Option<Duration>,
}

impl HttpModelClient {
    /// One attempt: send, read the stream, forward chunks.
    async fn attempt(
        &self,
        request: &ModelRequest,
        sink: &mpsc::Sender<ModelChunk>,
    ) -> std::result::Result<ModelResponse, AttemptError> {
        let response = self.send_once(request).await?;
        self.pump(response, sink).await
    }

    async fn send_once(
        &self,
        request: &ModelRequest,
    ) -> std::result::Result<reqwest::Response, AttemptError> {
        let family = Family::of(&self.config);
        let body = self
            .body_for(&family, request)
            .map_err(|error| AttemptError {
                error,
                kind: FailureKind::Permanent,
                published: false,
                retry_after: None,
            })?;
        // No reqwest request-level `timeout()`: that is a total deadline that
        // covers the body read and used to cut off long SSE streams. Bound only
        // the wait for response headers here; `pump` bounds each body read.
        let mut builder = self.client.post(self.config.request_url()).json(&body);
        for (name, value) in family.headers(&self.config) {
            builder = builder.header(name, value);
        }
        let send_result = match tokio::time::timeout(self.timeout, builder.send()).await {
            Ok(result) => result,
            Err(_) => {
                return Err(AttemptError {
                    kind: FailureKind::Transient,
                    error: ArkError::model(format!(
                        "model request timed out after {:?} waiting for response headers from {} {} ({})",
                        self.timeout,
                        self.config.provider_id,
                        self.config.model,
                        self.config.request_url()
                    )),
                    published: false,
                    retry_after: None,
                });
            }
        };
        let response = send_result.map_err(|error| AttemptError {
            // A transport error is worth another attempt unless it is a URL
            // problem, which repeating cannot fix.
            kind: classify_transport(&error),
            error: ArkError::model(format!(
                "model request failed for {} {} ({}): {error}",
                self.config.provider_id,
                self.config.model,
                self.config.request_url()
            )),
            published: false,
            retry_after: None,
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        // Read the provider-advised wait before the body read consumes the
        // response, then a bounded amount of the body so the message explains
        // the 4xx/5xx without dumping a whole HTML error page into the log.
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string());
        let retry_after = retry_after
            .and_then(|value| parse_retry_after(Some(&value), std::time::SystemTime::now()));
        // Removing the reqwest total deadline also removed its bound on this
        // diagnostic read, so cap it explicitly: a hung error body must not
        // wedge the retry loop.
        let text = match tokio::time::timeout(self.timeout, response.text()).await {
            Ok(Ok(text)) => text,
            _ => String::new(),
        };
        let summary = ark_common::text::truncate_utf8(&text, 512).to_string();
        Err(AttemptError {
            kind: FailureKind::from_status(status.as_u16()),
            error: ArkError::new(
                ArkErrorCode::ModelRequestFailed,
                format!(
                    "model request failed for {} {} at {} with HTTP {}",
                    self.config.provider_id,
                    self.config.model,
                    self.config.request_url(),
                    status.as_u16()
                ),
            )
            .with_upstream_status(status.as_u16())
            .with_detail(summary),
            published: false,
            retry_after,
        })
    }

    /// Read the SSE body and forward chunks as they are parsed.
    async fn pump(
        &self,
        response: reqwest::Response,
        sink: &mpsc::Sender<ModelChunk>,
    ) -> std::result::Result<ModelResponse, AttemptError> {
        let mut stream = response.bytes_stream();
        let mut sse = SseParser::new();
        let mut family = Family::of(&self.config);
        let mut accumulator = StreamAccumulator::new();
        let mut published = false;
        let mut text = String::new();
        let mut pending: Vec<u8> = Vec::new();

        loop {
            // Bound the *silence* between frames, not the length of the turn:
            // a live long-running reasoning stream must never be cut off by a
            // wall-clock deadline. Each received frame starts a fresh window.
            let chunk = match tokio::time::timeout(self.timeout, stream.next()).await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) => {
                    return Err(AttemptError {
                        error: ArkError::model(format!(
                            "model stream timed out after {:?} without new data for {} {}",
                            self.timeout, self.config.provider_id, self.config.model
                        )),
                        kind: FailureKind::Transient,
                        published,
                        retry_after: None,
                    });
                }
            };
            let bytes = chunk.map_err(|error| stream_failure(&error, published))?;
            // Bytes are decoded only down to the longest valid UTF-8 prefix:
            // a multi-byte codepoint split across TCP chunks must survive
            // into a later chunk, which per-chunk `from_utf8_lossy` would
            // have corrupted into U+FFFD.
            pending.extend_from_slice(&bytes);
            decode_utf8_prefix(&mut pending, &mut text);
            let drained = std::mem::take(&mut text);
            let (complete, rest) = split_at_last_complete_line(&drained);
            text.push_str(rest);
            for frame in sse.push(complete) {
                if accumulator.is_finished() {
                    // A terminal frame already arrived; bytes after it (e.g. a
                    // provider holding the connection open past `[DONE]`) are
                    // not part of the response and must not be published.
                    break;
                }
                let events = family.parse(&frame);
                published |= forward(&mut accumulator, events, sink).await?;
            }
            if accumulator.is_finished() {
                // `[DONE]` / `message_stop` / `response.completed` ends the
                // turn. Waiting for EOF here would let a provider that keeps
                // the connection alive trip the phase timeout, turning an
                // already-completed stream into a transient error and a retry.
                break;
            }
        }
        if !accumulator.is_finished() {
            if !pending.is_empty() {
                // EOF with a truncated trailing codepoint: keep the old lossy
                // tail behaviour instead of silently dropping those bytes.
                text.push_str(&String::from_utf8_lossy(&pending));
            }
            let drained = std::mem::take(&mut text);
            // Hand the remaining lines to the parser so `finish` below can still
            // flush a final frame whose blank line never arrived.
            for frame in sse.push(&drained) {
                let events = family.parse(&frame);
                published |= forward(&mut accumulator, events, sink).await?;
            }
            for frame in sse.finish() {
                let events = family.parse(&frame);
                published |= forward(&mut accumulator, events, sink).await?;
            }
        }
        let events = family.finish();
        published |= forward(&mut accumulator, events, sink).await?;
        // `finish` commits any tool call still being assembled. Forward those
        // chunks too: a provider that closes without a terminal frame would
        // otherwise report tool_calls in the response that the sink never saw.
        published |= flush_tail(&mut accumulator, sink).await?;
        let _ = published;
        Ok(accumulator.response())
    }
}

/// Decode the longest valid UTF-8 prefix of `pending` into `text`.
///
/// SSE bodies are UTF-8, but TCP chunks cut multi-byte codepoints at
/// arbitrary offsets. Per-chunk lossy decoding would turn those into
/// U+FFFD, so an incomplete trailing codepoint is kept for the next chunk.
/// Genuinely invalid bytes are replaced in place so one bad byte cannot
/// wedge the stream (matching the old lossy behaviour for that case).
fn decode_utf8_prefix(pending: &mut Vec<u8>, text: &mut String) {
    loop {
        match std::str::from_utf8(pending) {
            Ok(decoded) => {
                text.push_str(decoded);
                pending.clear();
                return;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(std::str::from_utf8(&pending[..valid]).unwrap_or(""));
                pending.drain(..valid);
                match error.error_len() {
                    // Truncated trailing codepoint: wait for the next chunk.
                    None => return,
                    // Truly invalid bytes: lossy-replace and keep decoding.
                    Some(bad) => {
                        pending.drain(..bad);
                        text.push('\u{FFFD}');
                    }
                }
            }
        }
    }
}

/// The consumer disappeared while a chunk was being published.
fn sink_closed() -> AttemptError {
    AttemptError {
        error: ArkError::new(ArkErrorCode::RuntimeUnavailable, "model sink closed"),
        kind: FailureKind::Permanent,
        published: true,
        retry_after: None,
    }
}

/// Publish tool calls that only become complete when the stream is finalized.
///
/// `StreamAccumulator::finish` returns the chunks assembled at EOF; forwarding
/// them keeps the sink and the returned `ModelResponse.tool_calls` in sync. A
/// terminal frame already flushed these calls, so this is a no-op there.
async fn flush_tail(
    accumulator: &mut StreamAccumulator,
    sink: &mpsc::Sender<ModelChunk>,
) -> std::result::Result<bool, AttemptError> {
    let mut published = false;
    for chunk in accumulator.finish() {
        published = true;
        if sink.send(chunk).await.is_err() {
            return Err(sink_closed());
        }
    }
    Ok(published)
}

/// Forward accumulated chunks, reporting whether anything was published.
async fn forward(
    accumulator: &mut StreamAccumulator,
    events: Vec<crate::domain::event::StreamEvent>,
    sink: &mpsc::Sender<ModelChunk>,
) -> std::result::Result<bool, AttemptError> {
    let mut published = false;
    for event in events {
        for chunk in accumulator.push(event) {
            published = true;
            if sink.send(chunk).await.is_err() {
                // The consumer stopped listening (interrupt): stop reading.
                return Err(sink_closed());
            }
        }
    }
    Ok(published)
}

/// Split a buffer at the last newline so a partial line is never parsed.
fn split_at_last_complete_line(buffer: &str) -> (&str, &str) {
    match buffer.rfind('\n') {
        Some(index) => (&buffer[..=index], &buffer[index + 1..]),
        None => ("", buffer),
    }
}

/// Decide whether repeating a **send** could help.
///
/// Used only for `RequestBuilder::send()` errors. Here a `Decode` kind means a
/// bad URL that repeating cannot fix, so it stays [`FailureKind::Permanent`].
fn classify_transport(error: &reqwest::Error) -> FailureKind {
    if error.is_timeout() || error.is_connect() || error.is_request() {
        FailureKind::Transient
    } else {
        FailureKind::Permanent
    }
}

/// Decide whether repeating a **body read** could help.
///
/// `Response::bytes_stream()` re-wraps every body error as `Kind::Decode`, so
/// here `is_decode()` means a truncated/reset stream, not an undecodable
/// payload (ark enables no content decoder). `is_timeout()` is the one signal
/// that sees through both wrappers: it walks the source chain down to
/// reqwest's `TimedOut`.
fn classify_stream(error: &reqwest::Error) -> FailureKind {
    if error.is_timeout() || error.is_connect() || error.is_body() || error.is_decode() {
        FailureKind::Transient
    } else {
        FailureKind::Permanent
    }
}

/// Turn a body-stream error into an attempt failure with an honest message.
///
/// reqwest labels every `bytes_stream()` error "error decoding response body",
/// which made a plain timeout look like a payload bug. Classify by the
/// underlying cause instead, and derive the retry kind rather than hardcoding
/// it in the `map_err` closure.
fn stream_failure(error: &reqwest::Error, published: bool) -> AttemptError {
    let message = if error.is_timeout() {
        "model stream timed out while waiting for data".to_string()
    } else {
        format!("model stream broke: {error}")
    };
    AttemptError {
        error: ArkError::model(message),
        kind: classify_stream(error),
        published,
        // Mid-stream there is no fresh Retry-After to read.
        retry_after: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::endpoint::EndpointFormat;
    use ark_ports::ModelMessage;

    fn config() -> ProviderConfig {
        ProviderConfig::new("deepseek", "deepseek-chat")
            .with_base_url("https://api.deepseek.com")
            .with_api_key("k")
    }

    fn request() -> ModelRequest {
        ModelRequest {
            messages: vec![ModelMessage::user("hi")],
            ..Default::default()
        }
    }

    #[test]
    fn url_is_resolved_from_the_provider_config() {
        let client = HttpModelClient::with_config(config());
        assert_eq!(
            client.request_url(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(client.provider_id(), "deepseek");
        assert_eq!(client.model(), "deepseek-chat");
    }

    #[test]
    fn each_format_resolves_its_own_path() {
        for (format, expected) in [
            (
                EndpointFormat::OpenAiChat,
                "https://api.deepseek.com/v1/chat/completions",
            ),
            (
                EndpointFormat::OpenAiResponses,
                "https://api.deepseek.com/v1/responses",
            ),
            (
                EndpointFormat::AnthropicMessages,
                "https://api.anthropic.com/v1/messages",
            ),
        ] {
            let base = if format == EndpointFormat::AnthropicMessages {
                "https://api.anthropic.com"
            } else {
                "https://api.deepseek.com"
            };
            let client = HttpModelClient::with_config(
                ProviderConfig::new("p", "m")
                    .with_base_url(base)
                    .with_format(format),
            );
            assert_eq!(client.request_url(), expected);
        }
    }

    #[test]
    fn body_matches_the_configured_family() {
        let client = HttpModelClient::with_config(config());
        let family = Family::of(client.config());
        let body = client.body_for(&family, &request()).unwrap();
        assert_eq!(body["model"], "deepseek-chat");
        assert_eq!(body["stream"], true);
    }

    #[tokio::test]
    async fn chat_request_sends_content_type_once() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4_096];
            let count = socket.read(&mut request).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 14\r\nConnection: close\r\n\r\ndata: [DONE]\n\n",
                )
                .unwrap();
            String::from_utf8_lossy(&request[..count]).into_owned()
        });
        let client = HttpModelClient::with_config(
            ProviderConfig::new("test", "test-model")
                .with_base_url(format!("http://{address}"))
                .with_api_key("test-key"),
        );
        let (sink, _drain) = mpsc::channel(1);

        client.stream(request(), sink).await.unwrap();

        let received = server.join().unwrap().to_ascii_lowercase();
        assert_eq!(
            received.matches("content-type: application/json").count(),
            1,
            "request contained duplicate content-type headers:\n{received}"
        );
    }

    /// A 429 with `Retry-After: 1` must wait ≈1s (the provider-advised delay),
    /// not the ≈1ms exponential fallback — and a `Retry-After: 0` must fall
    /// back to the backoff schedule instead of bypassing `rate_limit_floor`
    /// (same rule as smart-ark's `computeRetryDelay`: only adopt a value > 0).
    #[tokio::test]
    async fn rate_limited_response_is_retried_honouring_retry_after() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Instant;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let responses: [&[u8]; 2] = [
                b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 14\r\nConnection: close\r\n\r\ndata: [DONE]\n\n",
            ];
            for response in responses {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4_096];
                let _ = socket.read(&mut request).unwrap();
                socket.write_all(response).unwrap();
            }
        });
        let client = HttpModelClient::with_config(
            ProviderConfig::new("test", "test-model")
                .with_base_url(format!("http://{address}"))
                .with_api_key("test-key"),
        )
        .with_retry(RetryPolicy {
            max_attempts: 4,
            base: Duration::from_millis(1),
            max_delay: Duration::from_secs(30),
            rate_limit_floor: Duration::from_millis(1),
        });
        let (sink, _drain) = mpsc::channel(1);

        let started = Instant::now();
        let response = client.stream(request(), sink).await.unwrap();
        let elapsed = started.elapsed();

        assert_eq!(response.text, "", "the [DONE]-only stream yields no text");
        assert!(
            elapsed >= Duration::from_millis(700),
            "the retry must have waited the provider-advised ≈1s, took {elapsed:?}"
        );
        server.join().unwrap();
    }

    /// A stream that published text and then breaks mid-flight must surface as
    /// an error **without a second request**: retrying after the sink already
    /// received deltas would duplicate text. The body uses chunked framing and
    /// the server closes before the terminating chunk, so the truncation is a
    /// decoder error (a bare EOF with no framing would read as a complete —
    /// if sentinel-less — body instead).
    #[tokio::test]
    async fn a_failure_after_published_deltas_is_never_retried() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = {
            let listener = listener.try_clone().unwrap();
            std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4_096];
                let _ = socket.read(&mut request).unwrap();
                // One text-delta chunk, then close without the terminating
                // `0\r\n\r\n`: the stream breaks after content already
                // reached the sink.
                let frame = "data: {\"choices\":[{\"delta\":{\"content\":\"par\"}}]}\n\n";
                let body = format!("{:x}\r\n{frame}\r\n", frame.len());
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\n\
                             Content-Type: text/event-stream\r\n\
                             Transfer-Encoding: chunked\r\n\
                             Connection: close\r\n\
                             \r\n\
                             {body}"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                drop(socket);
            })
        };
        // A tiny base keeps a hypothetical (regressed) retry inside the
        // ~200ms accept-poll window below; the correct path never retries
        // regardless of the policy.
        let client = HttpModelClient::with_config(
            ProviderConfig::new("test", "test-model")
                .with_base_url(format!("http://{address}"))
                .with_api_key("test-key"),
        )
        .with_retry(RetryPolicy {
            max_attempts: 4,
            base: Duration::from_millis(1),
            max_delay: Duration::from_secs(30),
            rate_limit_floor: Duration::from_millis(1),
        });
        let (sink, mut drain) = mpsc::channel(4);

        let result = client.stream(request(), sink).await;
        server.join().unwrap();

        let error = result.expect_err("a truncated stream must fail");
        assert!(error.to_string().contains("model stream broke"));

        // The truncated delta was already published to the sink.
        let chunk = drain.recv().await.expect("published chunk");
        assert!(matches!(chunk, ModelChunk::TextDelta { .. }));

        // No second request: poll `accept` instead of blocking on it, so a
        // regression retries and fails this assertion in ~200ms.
        listener
            .set_nonblocking(true)
            .expect("listener switched to polling");
        let mut second_connection = false;
        for _ in 0..20 {
            if listener.accept().is_ok() {
                second_connection = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !second_connection,
            "a published-then-broken stream must not trigger a retry request"
        );
    }

    /// A multi-byte codepoint cut by a TCP chunk boundary must be re-assembled
    /// from the next chunk instead of becoming U+FFFD. Regression: the pump
    /// used to `from_utf8_lossy` every chunk independently.
    #[tokio::test]
    async fn codepoint_split_across_chunks_is_reassembled() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        // Cut two bytes into the 3-byte "中" (E4 B8 | AD) so the first chunk
        // ends with a truncated codepoint.
        let frame = "data: {\"choices\":[{\"delta\":{\"content\":\"中\"}}]}\n\n";
        let bytes = frame.as_bytes();
        let cut = frame.find('中').unwrap() + 2;
        let mut wire = Vec::new();
        for piece in [&bytes[..cut], &bytes[cut..]] {
            wire.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
            wire.extend_from_slice(piece);
            wire.extend_from_slice(b"\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4_096];
            let _ = socket.read(&mut request).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            socket.write_all(&wire).unwrap();
        });
        let client = HttpModelClient::with_config(
            ProviderConfig::new("test", "test-model")
                .with_base_url(format!("http://{address}"))
                .with_api_key("test-key"),
        );
        let (sink, mut drain) = mpsc::channel(16);

        client.stream(request(), sink).await.unwrap();
        server.join().unwrap();

        let mut text = String::new();
        while let Ok(chunk) = drain.try_recv() {
            if let ModelChunk::TextDelta { delta } = chunk {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "中", "split codepoint must not become U+FFFD");
    }

    #[test]
    fn truncated_trailing_codepoint_waits_for_the_next_chunk() {
        let full = "中".as_bytes();
        let mut text = String::new();
        let mut pending: Vec<u8> = full[..2].to_vec();
        decode_utf8_prefix(&mut pending, &mut text);
        assert!(text.is_empty(), "nothing may be emitted for a partial char");
        assert_eq!(pending, full[..2].to_vec());

        pending.extend_from_slice(&full[2..]);
        decode_utf8_prefix(&mut pending, &mut text);
        assert_eq!(text, "中");
        assert!(pending.is_empty());
    }

    #[test]
    fn four_byte_emoji_split_across_chunks_survives() {
        let bytes = "\u{1F600}".as_bytes().to_vec();
        let mut text = String::new();
        let mut pending: Vec<u8> = bytes[..3].to_vec();
        decode_utf8_prefix(&mut pending, &mut text);
        assert!(text.is_empty());
        pending.extend_from_slice(&bytes[3..]);
        decode_utf8_prefix(&mut pending, &mut text);
        assert_eq!(text, "\u{1F600}");
    }

    #[test]
    fn invalid_bytes_are_replaced_without_stalling() {
        // A genuinely invalid byte must be replaced in place and decoding
        // continue, or one bad byte would wedge the whole stream.
        let mut text = String::new();
        let mut pending: Vec<u8> = b"a\xFFb".to_vec();
        decode_utf8_prefix(&mut pending, &mut text);
        assert_eq!(text, "a\u{FFFD}b");
        assert!(pending.is_empty());
    }

    #[test]
    fn partial_lines_are_held_back() {
        let (complete, rest) = split_at_last_complete_line("data: a\ndata: par");
        assert_eq!(complete, "data: a\n");
        assert_eq!(rest, "data: par");
    }

    #[test]
    fn a_buffer_without_a_newline_is_all_rest() {
        let (complete, rest) = split_at_last_complete_line("data: a");
        assert!(complete.is_empty());
        assert_eq!(rest, "data: a");
    }

    /// Regression for the 120s cut-off: a live stream that keeps delivering
    /// frames must survive well past the per-phase bound. The old reqwest
    /// request-level `timeout()` was a *total* deadline covering the body, so
    /// it truncated this stream at `timeout` even though data never stopped.
    #[tokio::test]
    async fn a_live_long_stream_is_not_cut_off_by_the_phase_bound() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        // 8 frames x 30ms = ~240ms total, each gap well inside the 120ms phase
        // bound. Under the old total deadline the stream died at 120ms.
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4_096];
            let _ = socket.read(&mut request).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            for i in 0..8 {
                std::thread::sleep(Duration::from_millis(30));
                let frame =
                    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"x{i}\"}}}}]}}\n\n");
                socket
                    .write_all(format!("{:x}\r\n{frame}\r\n", frame.len()).as_bytes())
                    .unwrap();
                let _ = socket.flush();
            }
            socket.write_all(b"0\r\n\r\n").unwrap();
        });
        let client = HttpModelClient::with_config(
            ProviderConfig::new("test", "test-model")
                .with_base_url(format!("http://{address}"))
                .with_api_key("test-key"),
        )
        .with_retry(RetryPolicy::none())
        .with_timeout(Duration::from_millis(120));
        let (sink, _drain) = mpsc::channel(64);

        let response = client
            .stream(request(), sink)
            .await
            .expect("a stream with continuous frames must not be cut off");
        server.join().unwrap();

        assert_eq!(response.text, "x0x1x2x3x4x5x6x7");
    }

    /// A connection that goes quiet mid-stream must fail with an honest
    /// "timed out" message, never the old misleading "decoding" wording.
    #[tokio::test]
    async fn a_stalled_stream_reports_a_timeout_not_a_decode_failure() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4_096];
            let _ = socket.read(&mut request).unwrap();
            // Headers + one frame, then stall: the socket stays open so this is
            // a true idle stall, not a truncation. Unblocks when the client
            // gives up and closes the connection (capped at 2s).
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let frame = "data: {\"choices\":[{\"delta\":{\"content\":\"par\"}}]}\n\n";
            socket
                .write_all(format!("{:x}\r\n{frame}\r\n", frame.len()).as_bytes())
                .unwrap();
            let _ = socket.flush();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut probe = [0_u8; 1];
            let _ = socket.read(&mut probe);
        });
        let client = HttpModelClient::with_config(
            ProviderConfig::new("test", "test-model")
                .with_base_url(format!("http://{address}"))
                .with_api_key("test-key"),
        )
        .with_retry(RetryPolicy::none())
        .with_timeout(Duration::from_millis(60));
        let (sink, _drain) = mpsc::channel(16);

        let error = client
            .stream(request(), sink)
            .await
            .expect_err("a stalled stream must fail");
        server.join().unwrap();

        let message = error.to_string();
        assert!(message.contains("timed out"), "unexpected: {message}");
        assert!(
            !message.contains("decoding"),
            "a timeout must not be labelled a decode failure: {message}"
        );
    }

    /// reqwest's own read timeout produces `body(TimedOut)` that
    /// `bytes_stream()` re-wraps as `Kind::Decode`. `is_timeout()` must still
    /// see through that wrapper, or the diagnostic regresses to "decoding".
    #[tokio::test]
    async fn a_reqwest_read_timeout_is_still_reported_as_a_timeout() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4_096];
            let _ = socket.read(&mut request).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let _ = socket.flush();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut probe = [0_u8; 1];
            let _ = socket.read(&mut probe);
        });
        let transport = reqwest::Client::builder()
            .read_timeout(Duration::from_millis(60))
            .build()
            .unwrap();
        let client = HttpModelClient::new(
            ProviderConfig::new("test", "test-model")
                .with_base_url(format!("http://{address}"))
                .with_api_key("test-key"),
            transport,
        )
        .with_retry(RetryPolicy::none())
        // Keep our idle bound far above reqwest's so the reqwest error wins.
        .with_timeout(Duration::from_secs(5));
        let (sink, _drain) = mpsc::channel(16);

        let error = client
            .stream(request(), sink)
            .await
            .expect_err("a read timeout must fail");
        server.join().unwrap();

        let message = error.to_string();
        assert!(
            message.contains("timed out"),
            "reqwest timeout must be labelled as such: {message}"
        );
        assert!(
            !message.contains("decoding"),
            "must not mislabel a timeout as a decode error: {message}"
        );
    }

    #[test]
    fn builder_options_are_kept() {
        let client = HttpModelClient::with_config(config())
            .with_retry(RetryPolicy::none())
            .with_timeout(Duration::from_millis(5));
        assert_eq!(client.timeout, Duration::from_millis(5));
        assert_eq!(client.retry.max_attempts, 1);
    }

    #[test]
    fn debug_output_points_at_the_endpoint() {
        let client = HttpModelClient::with_config(config());
        let text = format!("{client:?}");
        assert!(text.contains("deepseek"));
        assert!(text.contains("chat/completions"));
    }
}
