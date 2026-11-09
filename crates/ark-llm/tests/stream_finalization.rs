//! Regression tests for stream finalization at the transport boundary.
//!
//! These live outside `service/http.rs` (already over the 1000-line limit) and
//! use only the public `ModelClient` surface plus a local TCP server, so they
//! pin the two ways a provider can end a turn without relying on internals.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use ark_llm::{HttpModelClient, ProviderConfig, RetryPolicy};
use ark_ports::{ModelChunk, ModelClient, ModelMessage, ModelRequest};
use tokio::sync::mpsc;

fn request() -> ModelRequest {
    ModelRequest {
        messages: vec![ModelMessage::user("hi")],
        ..Default::default()
    }
}

fn client(address: std::net::SocketAddr) -> HttpModelClient {
    HttpModelClient::with_config(
        ProviderConfig::new("test", "test-model")
            .with_base_url(format!("http://{address}"))
            .with_api_key("test-key"),
    )
    .with_retry(RetryPolicy::none())
    .with_timeout(Duration::from_millis(150))
}

/// A provider that sends `[DONE]` but keeps the connection open must not turn a
/// completed turn into a phase timeout (and a duplicate request on retry).
#[tokio::test]
async fn a_terminal_frame_ends_the_stream_without_waiting_for_eof() {
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
        // One text frame followed by the sentinel, deliberately in one chunk so
        // the pump cannot rely on a byte boundary to stop after `[DONE]`.
        let frames = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
        socket
            .write_all(format!("{:x}\r\n{frames}\r\n", frames.len()).as_bytes())
            .unwrap();
        let _ = socket.flush();
        // No terminating chunk and no close: EOF never arrives. The server only
        // unblocks when the client drops the connection after a completed turn.
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut probe = [0_u8; 1];
        let _ = socket.read(&mut probe);
    });
    let client = client(address);
    let (sink, mut drain) = mpsc::channel(16);

    let response = client
        .stream(request(), sink)
        .await
        .expect("[DONE] must complete the turn while the socket is still open");
    server.join().unwrap();

    assert_eq!(response.text, "ok");
    let mut chunks = Vec::new();
    while let Ok(chunk) = drain.try_recv() {
        chunks.push(chunk);
    }
    assert!(chunks
        .iter()
        .any(|chunk| matches!(chunk, ModelChunk::TextDelta { .. })));
}

/// A stream that closes with no sentinel still has to publish the tool call it
/// assembled: `response.tool_calls` and the sink must not diverge.
#[tokio::test]
async fn eof_without_a_terminal_frame_still_publishes_assembled_tool_calls() {
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
        let frame = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}}]}}]}\n\n";
        socket
            .write_all(format!("{:x}\r\n{frame}\r\n0\r\n\r\n", frame.len()).as_bytes())
            .unwrap();
        let _ = socket.flush();
        drop(socket);
    });
    let client = client(address);
    let (sink, mut drain) = mpsc::channel(16);

    let response = client
        .stream(request(), sink)
        .await
        .expect("an EOF without [DONE] is still a usable response");
    server.join().unwrap();

    assert_eq!(response.tool_calls.len(), 1, "the response owns the call");
    assert_eq!(response.tool_calls[0].1, "read");
    let mut chunks = Vec::new();
    while let Ok(chunk) = drain.try_recv() {
        chunks.push(chunk);
    }
    assert!(
        chunks
            .iter()
            .any(|chunk| matches!(chunk, ModelChunk::ToolCall { tool, .. } if tool == "read")),
        "the sink must see the same tool call the response reports"
    );
}
