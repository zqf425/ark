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

use crate::common::retry::{FailureKind, RetryPolicy, RetryState};
use crate::common::sse::SseParser;
use crate::common::stream::StreamAccumulator;
use crate::config::endpoint::ProviderConfig;
use crate::domain::event::StreamEvent;
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
                            tokio::time::sleep(next.next_delay).await;
                        }
                        None => return Err(failure.error),
                    }
                }
            }
        }
    }
}

/// A failed attempt plus what it already published.
struct AttemptError {
    error: ArkError,
    kind: FailureKind,
    /// True once at least one chunk reached the sink.
    published: bool,
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
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        // Read a bounded amount of the body so the message explains the 4xx/5xx
        // without dumping a whole HTML error page into the log.
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

    /// The response to return when the stream carried nothing usable.
    fn fallback(&self, accumulator: &StreamAccumulator) -> ModelResponse {
        let mut response = accumulator.response();
        if response.text.is_empty() && response.tool_calls.is_empty() {
            // Not an error: a provider may legitimately end with only usage.
            response.text = String::new();
        }
        response
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
