//! Bus-level tests: real loopback socket, no mocks of the transport.
//!
//! These prove the properties an operator depends on: the host owns the
//! `User-Agent`, the allowlist denies before a socket is opened, and a redirect
//! to a non-allowlisted host is refused instead of followed.

use super::*;
use crate::domain::HostAllowlist;
use crate::domain::MemoryDiagnostics;
use crate::service::prepare::prepare;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn bus(policy: HttpPolicy) -> (Arc<HttpBus>, Arc<MemoryDiagnostics>) {
    let sink = Arc::new(MemoryDiagnostics::new(64));
    let bus = HttpBus::new(policy, Arc::new(CredentialVault::new()), sink.clone()).unwrap();
    (Arc::new(bus), sink)
}

fn permissive() -> HttpPolicy {
    HttpPolicy {
        allow_insecure_http: true,
        retry: crate::config::RetryPolicy {
            max_attempts: 1,
            base_ms: 1,
            max_ms: 1,
        },
        rate: crate::config::RateLimitPolicy {
            capacity: 0.0,
            refill_per_sec: 0.0,
            per_host_capacity: 0.0,
            per_host_refill_per_sec: 0.0,
        },
        max_timeout_ms: 3_000,
        default_timeout_ms: 3_000,
        follow_redirects: false,
        ..HttpPolicy::default()
    }
}

/// One-shot server: replies with `response` after reading the request head.
async fn serve_once(response: String) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://127.0.0.1:{}{}", addr.port(), "/v1/ping");
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = vec![0u8; 4096];
        let read = socket.read(&mut buffer).await.unwrap();
        let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        request
    });
    (url, handle)
}

fn ok_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

#[tokio::test]
async fn host_owned_headers_reach_the_wire() {
    let (url, server) = serve_once(ok_response("{\"ok\":true}")).await;
    let (bus, sink) = bus(permissive());
    bus.register_plugin("ark.example.jira", "1.2.3", &["127.0.0.1".to_string()])
        .unwrap();

    let request = OutboundRequest {
        method: "GET".into(),
        url: url.clone(),
        headers: [("accept".to_string(), "application/json".to_string())]
            .into_iter()
            .collect(),
        ..OutboundRequest::default()
    };
    let response = bus.request("ark.example.jira", request).await.unwrap();
    assert_eq!(response.status, 200);
    assert!(response.body.contains("\"ok\":true"));

    let seen = server.await.unwrap();
    assert!(seen.starts_with("GET /v1/ping"), "{seen}");
    assert!(
        seen.to_ascii_lowercase()
            .contains("user-agent: ark-plugin/ark.example.jira@1.2.3"),
        "host must own the User-Agent: {seen}"
    );
    assert!(seen
        .to_ascii_lowercase()
        .contains("x-plugin-id: ark.example.jira"));
    assert!(seen.to_ascii_lowercase().contains("x-ark-trace-id: tr-"));

    let spans = sink.records();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].kind, "http_span");
    assert_eq!(spans[0].status, Some(200));
}

#[tokio::test]
async fn off_allowlist_host_is_denied_without_opening_a_socket() {
    let (bus, sink) = bus(permissive());
    bus.register_plugin(
        "ark.example.jira",
        "1.0.0",
        &["api.example.com".to_string()],
    )
    .unwrap();
    let request = OutboundRequest {
        method: "GET".into(),
        url: "https://169.254.169.254/latest/meta-data".into(),
        ..OutboundRequest::default()
    };
    let err = bus.request("ark.example.jira", request).await.unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("host_not_allowed"));

    let records = sink.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "http_denied");
    assert_eq!(records[0].reason, Some(HttpDenialReason::HostNotAllowed));
    assert!(records[0].redacted_url.contains("169.254.169.254"));
}

#[tokio::test]
async fn unregistered_plugin_cannot_use_the_bus() {
    let (bus, sink) = bus(permissive());
    let request = OutboundRequest {
        method: "GET".into(),
        url: "http://127.0.0.1:1/x".into(),
        ..OutboundRequest::default()
    };
    let err = bus
        .request("ark.example.unknown", request)
        .await
        .unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("empty_allowlist"));
    assert_eq!(sink.records()[0].kind, "http_denied");
}

#[tokio::test]
async fn plugin_cannot_forge_the_user_agent() {
    let (bus, sink) = bus(permissive());
    bus.register_plugin("ark.example.jira", "1.0.0", &["127.0.0.1".to_string()])
        .unwrap();
    let mut headers = ark_ports::HeaderMap::new();
    headers.insert("user-agent".into(), "evil/1.0".into());
    let request = OutboundRequest {
        method: "GET".into(),
        url: "http://127.0.0.1:1/x".into(),
        headers,
        ..OutboundRequest::default()
    };
    let err = bus.request("ark.example.jira", request).await.unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("reserved_header"));
    assert_eq!(sink.records()[0].kind, "http_denied");
}

#[tokio::test]
async fn cross_host_redirect_is_refused_by_the_allowlist() {
    let (url, server) = serve_once(
        "HTTP/1.1 302 Found\r\nLocation: https://evil.test/steal\r\nContent-Length: 0\r\n\r\n"
            .to_string(),
    )
    .await;
    let policy = HttpPolicy {
        follow_redirects: true,
        ..permissive()
    };
    let (bus, sink) = bus(policy);
    bus.register_plugin("ark.example.jira", "1.0.0", &["127.0.0.1".to_string()])
        .unwrap();

    let request = OutboundRequest {
        method: "GET".into(),
        url,
        ..OutboundRequest::default()
    };
    let err = bus.request("ark.example.jira", request).await.unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("host_not_allowed"));
    let _ = server.await;

    let denied = sink
        .records()
        .into_iter()
        .find(|record| record.kind == "http_denied")
        .expect("redirect must be audited");
    assert_eq!(denied.reason, Some(HttpDenialReason::HostNotAllowed));
}

#[tokio::test]
async fn rate_limited_plugin_gets_a_denial_record() {
    let policy = HttpPolicy {
        rate: crate::config::RateLimitPolicy {
            capacity: 1.0,
            refill_per_sec: 0.0001,
            per_host_capacity: 0.0,
            per_host_refill_per_sec: 0.0,
        },
        ..permissive()
    };
    let (url, server) = serve_once(ok_response("{}")).await;
    let (bus, sink) = bus(policy);
    bus.register_plugin("ark.example.jira", "1.0.0", &["127.0.0.1".to_string()])
        .unwrap();

    let request = OutboundRequest {
        method: "GET".into(),
        url: url.clone(),
        ..OutboundRequest::default()
    };
    bus.request("ark.example.jira", request.clone())
        .await
        .unwrap();
    let _ = server.await;
    let err = bus.request("ark.example.jira", request).await.unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("rate_limited"));
    assert!(sink
        .records()
        .iter()
        .any(|record| record.reason == Some(HttpDenialReason::RateLimited)));
}

#[test]
fn credential_is_injected_but_never_logged() {
    let vault = CredentialVault::new();
    vault
        .store_for("ark.example.jira", "token", "plain-secret")
        .unwrap();
    let allowlist = HostAllowlist::parse(&["api.example.com".to_string()]).unwrap();
    let request = OutboundRequest {
        method: "GET".into(),
        url: "https://api.example.com/v1".into(),
        credential: Some("token".into()),
        ..OutboundRequest::default()
    };
    let prepared = prepare(
        &HttpPolicy::default(),
        "ark.example.jira",
        "1.0.0",
        &allowlist,
        &vault,
        &request,
    )
    .unwrap();
    assert_eq!(prepared.headers["authorization"], "Bearer plain-secret");
    let label = prepared.credential_label.clone().unwrap();
    assert!(!label.contains("plain-secret"));
    let span = HttpDiagnostic {
        kind: "http_span".into(),
        plugin_id: "ark.example.jira".into(),
        host: prepared.host.clone(),
        method: prepared.method.clone(),
        redacted_url: redact_url(&prepared.url),
        reason: None,
        detail: Some(label),
        status: Some(200),
        duration_ms: Some(1),
        attempts: Some(1),
    };
    let json = serde_json::to_string(&span).unwrap();
    assert!(
        !json.contains("plain-secret"),
        "secret leaked into the span: {json}"
    );
}
