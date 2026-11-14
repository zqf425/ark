//! Unit tests for `HttpModelClient`, split out of `http.rs` so the production
//! file stays within the AGENTS §4 file-size limit. Declared as a descendant
//! of `http.rs`'s `tests` module via `#[path]`; the suite imports exactly what
//! it needs, so the two `use` lists stay explicit.

use crate::common::retry::RetryPolicy;
use crate::config::endpoint::{EndpointFormat, ProviderConfig};
use crate::provider::route::Family;
use crate::service::http::{decode_utf8_prefix, split_at_last_complete_line, HttpModelClient};
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
