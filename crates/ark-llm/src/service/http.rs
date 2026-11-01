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

/// HTTP-backed [`ModelClient`].
pub struct HttpModelClient {
    config: ProviderConfig,
    client: reqwest::Client,
    retry: RetryPolicy,
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
            timeout: Duration::from_secs(120),
        }
    }

    /// Build a client with a default `reqwest::Client`.
    pub fn with_config(config: ProviderConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self::new(config, client)
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

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
        let mut builder = self
            .client
            .post(self.config.request_url())
            .timeout(self.timeout)
            .json(&body);
        for (name, value) in family.headers(&self.config) {
            builder = builder.header(name, value);
        }
        let response = builder.send().await.map_err(|error| AttemptError {
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
        let text = response.text().await.unwrap_or_default();
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
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|error| AttemptError {
                error: ArkError::model(format!("model stream broke: {error}")),
                kind: FailureKind::Transient,
                published,
                // Mid-stream there is no fresh Retry-After to read.
                retry_after: None,
            })?;
            // SSE is UTF-8 text; a multi-byte codepoint split across chunks is
            // re-assembled because `from_utf8_lossy` would corrupt it. The
            // parser therefore receives complete lines only.
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            let drained = std::mem::take(&mut buffer);
            let (complete, rest) = split_at_last_complete_line(&drained);
            buffer.push_str(rest);
            for frame in sse.push(complete) {
                let events = family.parse(&frame);
                published |= forward(&mut accumulator, events, sink).await?;
            }
        }
        for frame in sse.finish() {
            let events = family.parse(&frame);
            published |= forward(&mut accumulator, events, sink).await?;
        }
        let events = family.finish();
        published |= forward(&mut accumulator, events, sink).await?;
        // `finish` commits any tool call still being assembled; `response` then
        // reports exactly what was published plus what was accumulated.
        let _ = accumulator.finish();
        let _ = published;
        Ok(accumulator.response())
    }
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
                return Err(AttemptError {
                    error: ArkError::new(ArkErrorCode::RuntimeUnavailable, "model sink closed"),
                    kind: FailureKind::Permanent,
                    published: true,
                    retry_after: None,
                });
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

fn classify_transport(error: &reqwest::Error) -> FailureKind {
    if error.is_timeout() || error.is_connect() || error.is_request() {
        FailureKind::Transient
    } else {
        FailureKind::Permanent
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
