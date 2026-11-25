//! Unit tests for `HttpModelClient`, split out of `http.rs` so the production
//! file stays within the AGENTS §4 file-size limit. Declared as a descendant
//! of `http.rs`'s `tests` module via `#[path]`; the suite imports exactly what
//! it needs, so the two `use` lists stay explicit.

use crate::common::retry::RetryPolicy;
use crate::config::endpoint::{EndpointFormat, ProviderConfig};
use crate::provider::route::Family;
use crate::service::http::{decode_utf8_prefix, HttpModelClient};
use ark_ports::{ModelChunk, ModelClient, ModelMessage, ModelRequest};
use std::time::Duration;
use tokio::sync::mpsc;

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
/// (same rule as the retry-delay computation: only adopt a value > 0).
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
    let done = "data: [DONE]\n\n";
    wire.extend_from_slice(format!("{:x}\r\n{done}\r\n", done.len()).as_bytes());
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
        let done = "data: [DONE]\n\n";
        socket
            .write_all(format!("{:x}\r\n{done}\r\n", done.len()).as_bytes())
            .unwrap();
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

fn serve_once(
    listener: std::net::TcpListener,
    response: &'static [u8],
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4_096];
        let _ = socket.read(&mut request).unwrap();
        let _ = socket.write_all(response);
    })
}

fn sse_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn client_at(address: std::net::SocketAddr, format: EndpointFormat) -> HttpModelClient {
    HttpModelClient::with_config(
        ProviderConfig::new("test", "test-model")
            .with_base_url(format!("http://{address}"))
            .with_format(format)
            .with_api_key("test-key"),
    )
    .with_retry(RetryPolicy::none())
    .with_timeout(Duration::from_millis(150))
}

async fn assert_stream_failure(format: EndpointFormat, body: &'static str, expected: &str) {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = serve_once(
        listener,
        Box::leak(response.into_bytes().into_boxed_slice()),
    );
    let (sink, _drain) = mpsc::channel(16);

    let error = client_at(address, format)
        .stream(request(), sink)
        .await
        .expect_err("a provider error frame must fail the request");
    server.join().unwrap();

    assert!(error.message.contains(expected), "unexpected: {error:?}");
}

#[tokio::test]
async fn http_200_chat_error_is_an_attempt_failure() {
    assert_stream_failure(
        EndpointFormat::OpenAiChat,
        "data: {\"error\":{\"message\":\"quota exhausted\"}}\n\n",
        "quota exhausted",
    )
    .await;
}

#[tokio::test]
async fn http_200_responses_failed_is_an_attempt_failure() {
    assert_stream_failure(
        EndpointFormat::OpenAiResponses,
        "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"model overloaded\"}}}\n\n",
        "model overloaded",
    )
    .await;
}

#[tokio::test]
async fn http_200_anthropic_error_is_an_attempt_failure() {
    assert_stream_failure(
        EndpointFormat::AnthropicMessages,
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"capacity unavailable\"}}\n\n",
        "capacity unavailable",
    )
    .await;
}

#[tokio::test]
async fn stream_error_after_a_delta_is_not_retried() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\ndata: {\"error\":{\"message\":\"upstream stopped\"}}\n\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = serve_once(
        listener.try_clone().unwrap(),
        Box::leak(response.into_bytes().into_boxed_slice()),
    );
    let client = client_at(address, EndpointFormat::OpenAiChat).with_retry(RetryPolicy {
        max_attempts: 4,
        base: Duration::from_millis(1),
        max_delay: Duration::from_millis(2),
        rate_limit_floor: Duration::from_millis(1),
    });
    let (sink, mut drain) = mpsc::channel(16);

    let error = client.stream(request(), sink).await.unwrap_err();
    server.join().unwrap();
    assert!(error.message.contains("upstream stopped"));
    assert!(matches!(
        drain.recv().await,
        Some(ModelChunk::TextDelta { .. })
    ));
    listener.set_nonblocking(true).unwrap();
    assert!(
        listener.accept().is_err(),
        "an error after published content must not retry"
    );
}

/// Role-only and ping frames publish no chunks. They must not clear the fact
/// that an earlier delta reached the sink, or a later provider error would
/// replay the request and duplicate already-visible text.
#[tokio::test]
async fn stream_error_after_delta_and_silent_frames_is_not_retried() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let server_requests = Arc::clone(&requests);
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        server_requests.fetch_add(1, Ordering::SeqCst);
        let mut request = [0_u8; 4_096];
        let _ = socket.read(&mut request).unwrap();
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "event: ping\ndata: {}\n\n",
            "data: {\"error\":{\"message\":\"upstream stopped\"}}\n\n",
        );
        socket.write_all(&sse_response(body)).unwrap();

        // A regressed retry arrives within a few milliseconds. Keep the
        // listener available briefly and answer it with visibly duplicate text.
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_millis(300);
        loop {
            match listener.accept() {
                Ok((mut retry, _)) => {
                    server_requests.fetch_add(1, Ordering::SeqCst);
                    let _ = retry.read(&mut request);
                    let retry_body = concat!(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"duplicate\"}}]}\n\n",
                        "data: [DONE]\n\n",
                    );
                    let _ = retry.write_all(&sse_response(retry_body));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("retry listener failed: {error}"),
            }
        }
    });
    let client = client_at(address, EndpointFormat::OpenAiChat).with_retry(RetryPolicy {
        max_attempts: 4,
        base: Duration::from_millis(1),
        max_delay: Duration::from_millis(2),
        rate_limit_floor: Duration::from_millis(1),
    });
    let (sink, mut drain) = mpsc::channel(16);

    let result = client.stream(request(), sink).await;
    server.join().unwrap();

    let error = result.expect_err("the provider error must remain terminal");
    assert!(error.message.contains("upstream stopped"));
    assert_eq!(requests.load(Ordering::SeqCst), 1, "must not retry");
    let mut text = String::new();
    while let Ok(chunk) = drain.try_recv() {
        if let ModelChunk::TextDelta { delta } = chunk {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "partial", "published text must not be replayed");
}

#[tokio::test]
async fn an_incomplete_frame_cannot_extend_its_deadline_with_more_bytes() {
    use std::io::Write;
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        use std::io::Read as _;
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4_096];
        let _ = socket.read(&mut request).unwrap();
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        for _ in 0..20 {
            let piece = b"data: x";
            if socket
                .write_all(format!("{:x}\r\n", piece.len()).as_bytes())
                .is_err()
                || socket.write_all(piece).is_err()
                || socket.write_all(b"\r\n").is_err()
            {
                break;
            }
            let _ = socket.flush();
            std::thread::sleep(Duration::from_millis(20));
        }
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut probe = [0_u8; 1];
        let _ = socket.read(&mut probe);
    });
    let client = client_at(address, EndpointFormat::OpenAiChat)
        .with_retry(RetryPolicy::none())
        .with_timeout(Duration::from_millis(60));
    let (sink, _drain) = mpsc::channel(16);

    let error = client.stream(request(), sink).await.unwrap_err();
    server.join().unwrap();
    assert!(error.message.contains("without a complete SSE frame"));
}

#[tokio::test]
async fn oversized_error_body_read_stops_after_the_diagnostic_limit() {
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
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        socket.write_all(&[b'x'; 1_024]).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(250)))
            .unwrap();
        let mut probe = [0_u8; 1];
        let _ = socket.read(&mut probe);
        let _ = socket.write_all(&[b'x'; 3_072]);
    });
    let (sink, _drain) = mpsc::channel(1);

    let started = std::time::Instant::now();
    let error = client_at(address, EndpointFormat::OpenAiChat)
        .with_timeout(Duration::from_secs(1))
        .stream(request(), sink)
        .await
        .unwrap_err();
    let elapsed = started.elapsed();
    server.join().unwrap();
    assert!(
        elapsed < Duration::from_millis(200),
        "client waited for the rest of the body: {elapsed:?}"
    );
    assert!(error.detail.is_some_and(|detail| detail.len() <= 512));
}
