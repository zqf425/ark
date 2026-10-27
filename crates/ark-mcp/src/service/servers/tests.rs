//! Tests for the external server registry.
//!
//! Two kinds of coverage:
//!
//! * **Pure registry behaviour** - validation, persistence, redaction, upsert
//!   semantics, delete. No process or socket involved.
//! * **Real connection path** - a minimal MCP server is served from a
//!   `std::net::TcpListener` on loopback and reached through the same
//!   `StreamableHttpTransport` -> `HttpBus` path production uses. That keeps the
//!   test honest without depending on `node`/`python` being installed, and without
//!   spawning a second test binary as a fake child process.
//!
//! The isolation test mixes a deliberately unspawnable stdio server with the
//! working HTTP one, which is exactly the "one bad server must not hide the
//! others" case.

use super::*;
use ark_contract::McpTransportKind;
use ark_ports::{SandboxDecision, ToolArgs, ToolContext};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ark-mcp-servers-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stdio_entry(id: &str, command: &str) -> McpServerEntry {
    McpServerEntry {
        id: id.to_string(),
        transport: McpTransportKind::Stdio,
        enabled: true,
        command: Some(command.to_string()),
        args: Vec::new(),
        env: BTreeMap::new(),
        url: None,
        headers: BTreeMap::new(),
        allow_tools: Vec::new(),
    }
}

fn http_entry(id: &str, url: &str) -> McpServerEntry {
    McpServerEntry {
        id: id.to_string(),
        transport: McpTransportKind::StreamableHttp,
        enabled: true,
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        url: Some(url.to_string()),
        headers: BTreeMap::new(),
        allow_tools: Vec::new(),
    }
}

fn tool_ctx() -> ToolContext {
    ToolContext {
        thread_id: ark_common::ThreadId::new(),
        turn_id: ark_common::TurnId::new(),
        workspace: std::env::temp_dir(),
        sandbox: SandboxDecision::ReadOnly,
        env: BTreeMap::new(),
    }
}

// ---------------------------------------------------------------------------
// minimal MCP server over loopback HTTP
// ---------------------------------------------------------------------------

/// Loopback MCP server exposing `echo` and a failing `boom` tool.
struct MockMcp {
    url: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockMcp {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr: SocketAddr = listener.local_addr().expect("local addr");
        listener.set_nonblocking(true).expect("nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // One thread per connection: serving inline made the
                        // accept loop block for the duration of a request, so a
                        // client that opened a second connection could time out
                        // and the test failed intermittently.
                        std::thread::spawn(move || {
                            let _ = serve(stream);
                        });
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url: format!("http://{addr}/mcp"),
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for MockMcp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 2048];
    let header_end = loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        raw.extend_from_slice(&chunk[..read]);
        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..header_end]).to_ascii_lowercase();
    let length = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = raw[header_end..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    let payload = String::from_utf8_lossy(&body).to_string();
    let response = respond(&payload);
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nauthorization-scheme: mock\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n",
        response.len()
    )?;
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// Answer one JSON-RPC message the way a small MCP server would.
fn respond(payload: &str) -> String {
    let Ok(message) = serde_json::from_str::<serde_json::Value>(payload) else {
        return "{}".to_string();
    };
    let Some(id) = message.get("id").cloned() else {
        // Notification: nothing to answer.
        return "{}".to_string();
    };
    let method = message
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or_default();
    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "serverInfo": { "name": "mock-mcp", "version": "0.0.1" },
            }
        })
        .to_string(),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "echo the arguments back",
                        "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } }
                    },
                    {
                        "name": "boom",
                        "description": "always fails",
                        "inputSchema": { "type": "object" }
                    }
                ]
            }
        })
        .to_string(),
        "tools/call" => call(id, &message),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "method not found" }
        })
        .to_string(),
    }
}

fn call(id: serde_json::Value, message: &serde_json::Value) -> String {
    let params = message
        .get("params")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or_default();
    if name == "boom" {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32603, "message": "remote boom" }
        })
        .to_string();
    }
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": arguments.to_string() }],
            "isError": false
        }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// validation and persistence
// ---------------------------------------------------------------------------

#[test]
fn upsert_rejects_bad_ids_commands_and_urls() {
    let registry = McpServerRegistry::new(temp_dir("validation"));

    for bad in ["", "  ", "has space", "star*", "a/b"] {
        let err = registry.upsert(stdio_entry(bad, "cmd")).unwrap_err();
        assert_eq!(
            err.code,
            ark_common::ArkErrorCode::ValidationError,
            "{bad:?}"
        );
    }

    let err = registry
        .upsert(stdio_entry("nocommand", "   "))
        .unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("invalid_command"));

    for bad_url in ["not a url", "file:///etc/passwd", "ftp://example.com/mcp"] {
        let err = registry.upsert(http_entry("remote", bad_url)).unwrap_err();
        assert_eq!(
            err.code,
            ark_common::ArkErrorCode::ValidationError,
            "{bad_url}"
        );
    }

    // Nothing invalid was persisted.
    assert!(registry.list().servers.is_empty());
}

#[test]
fn upsert_persists_without_bom_and_reloads_identically() {
    let dir = temp_dir("persist");
    let registry = McpServerRegistry::new(dir.clone());
    let mut entry = stdio_entry("jira", "npx");
    entry.args = vec!["-y".to_string(), "jira-mcp".to_string()];
    registry.upsert(entry.clone()).unwrap();

    assert_eq!(registry.data_path(), dir.join("mcp-servers.json"));
    let bytes = std::fs::read(registry.data_path()).unwrap();
    assert_ne!(
        bytes.first(),
        Some(&0xEF),
        "persisted file must not start with a BOM"
    );
    assert_eq!(bytes.last(), Some(&b'\n'));

    let reloaded = McpServerRegistry::new(dir);
    let views = reloaded.list().servers;
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].id, "jira");
    assert_eq!(views[0].transport, McpTransportKind::Stdio);
    assert!(
        !views[0].connected,
        "a freshly loaded registry must report not-yet-connected"
    );
}

#[test]
fn list_never_echoes_secret_values() {
    let dir = temp_dir("redaction");
    let registry = McpServerRegistry::new(dir);
    let mut entry = stdio_entry("secretive", "npx");
    entry
        .env
        .insert("API_TOKEN".to_string(), "s3cret-env-value".to_string());
    entry
        .headers
        .insert("x-api-key".to_string(), "s3cret-header-value".to_string());
    registry.upsert(entry).unwrap();

    let serialized = serde_json::to_string(&registry.list()).unwrap();
    assert!(
        !serialized.contains("s3cret-env-value"),
        "env value leaked: {serialized}"
    );
    assert!(
        !serialized.contains("s3cret-header-value"),
        "header value leaked: {serialized}"
    );
    assert!(
        !serialized.contains("\"env\""),
        "view must not expose env at all"
    );
    assert!(
        !serialized.contains("\"headers\""),
        "view must not expose headers at all"
    );
}

#[test]
fn upserting_the_same_id_updates_in_place() {
    let dir = temp_dir("upsert");
    let registry = McpServerRegistry::new(dir);
    registry.upsert(stdio_entry("dup", "npx")).unwrap();
    registry.upsert(stdio_entry("dup", "node")).unwrap();

    let views = registry.list().servers;
    assert_eq!(
        views.len(),
        1,
        "repeated upsert must not add a second entry"
    );

    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(registry.data_path()).unwrap()).unwrap();
    assert_eq!(on_disk["servers"].as_array().unwrap().len(), 1);
    assert_eq!(on_disk["servers"][0]["command"], "node");
}

#[test]
fn delete_unknown_id_is_a_not_found_error() {
    let registry = McpServerRegistry::new(temp_dir("delete"));
    registry.upsert(stdio_entry("keep", "npx")).unwrap();

    let err = registry.delete("missing").unwrap_err();
    assert_eq!(err.code, ark_common::ArkErrorCode::NotFound);
    assert!(err.message.contains("missing"));

    let remaining = registry.delete("keep").unwrap();
    assert!(remaining.servers.is_empty());
}

#[test]
fn two_ids_differing_only_by_case_are_refused() {
    let registry = McpServerRegistry::new(temp_dir("case"));
    registry.upsert(stdio_entry("Jira", "npx")).unwrap();
    let err = registry.upsert(stdio_entry("jira", "npx")).unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("mcp_server_id_conflict"));
    assert_eq!(registry.list().servers.len(), 1);
}

// ---------------------------------------------------------------------------
// live connection path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_and_connect_all_reach_a_live_server() {
    let server = MockMcp::start();
    let registry = McpServerRegistry::new(temp_dir("live"));
    registry.upsert(http_entry("mock", &server.url)).unwrap();

    let result = registry.test("mock").await;
    assert!(result.ok, "test failed: {}", result.message);
    assert!(
        result.tools.contains(&"mock.echo".to_string()),
        "{:?}",
        result.tools
    );
    assert!(
        result.tools.contains(&"mock.boom".to_string()),
        "{:?}",
        result.tools
    );
    assert!(result.tools.iter().all(|name| name.starts_with("mock.")));
    assert!(result.message.contains("2 tool(s)"), "{}", result.message);

    let connected = registry.list().servers;
    assert!(connected[0].connected);
    assert_eq!(connected[0].tool_count, 2);
    assert!(connected[0].last_error.is_none());
}

#[tokio::test]
async fn adapter_exposes_qualified_name_and_forwards_calls() {
    let server = MockMcp::start();
    let registry = McpServerRegistry::new(temp_dir("adapter"));
    registry.upsert(http_entry("mock", &server.url)).unwrap();
    let adapters = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let adapters = registry.connect_all().await;
            if adapters.len() == 2 {
                return adapters;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("mock MCP server must become reachable");

    let echo = adapters
        .iter()
        .find(|tool| tool.name() == "mock.echo")
        .expect("echo adapter");
    assert_eq!(echo.schema()["type"], "object");

    let output = echo
        .execute(&tool_ctx(), ToolArgs(json!({ "text": "hi" })))
        .await
        .unwrap();
    assert!(output.ok, "{}", output.content);
    assert!(
        output.content.contains("\"text\":\"hi\""),
        "{}",
        output.content
    );

    let boom = adapters
        .iter()
        .find(|tool| tool.name() == "mock.boom")
        .expect("boom adapter");
    let failed = boom
        .execute(&tool_ctx(), ToolArgs(json!({})))
        .await
        .unwrap();
    assert!(
        !failed.ok,
        "remote error must become a tool error, not an Err"
    );
    assert!(failed.content.contains("remote boom"), "{}", failed.content);
}

#[tokio::test]
async fn connect_all_isolates_an_unspawnable_server() {
    let server = MockMcp::start();
    let registry = McpServerRegistry::new(temp_dir("isolation"));
    registry
        .upsert(stdio_entry("broken", "ark-mcp-no-such-binary-xyz"))
        .unwrap();
    registry.upsert(http_entry("mock", &server.url)).unwrap();

    let adapters = registry.connect_all().await;
    let names: Vec<&str> = adapters.iter().map(|tool| tool.name()).collect();
    assert_eq!(
        names,
        vec!["mock.echo", "mock.boom"],
        "good server must still connect"
    );

    let views = registry.list().servers;
    let broken = views
        .iter()
        .find(|v| v.id == "broken")
        .expect("broken view");
    assert!(!broken.connected);
    let message = broken
        .last_error
        .clone()
        .expect("broken server must report an error");
    assert!(message.contains("failed to start the server"), "{message}");

    let good = views.iter().find(|v| v.id == "mock").expect("mock view");
    assert!(good.connected);
    assert!(good.last_error.is_none());
}

#[tokio::test]
async fn a_failing_connection_does_not_panic_and_is_reported() {
    let registry = McpServerRegistry::new(temp_dir("failure"));
    registry
        .upsert(stdio_entry("broken", "ark-mcp-no-such-binary-xyz"))
        .unwrap();

    let result = registry.test("broken").await;
    assert!(!result.ok);
    assert!(result.tools.is_empty());
    assert!(!result.message.is_empty());

    let missing = registry.test("never-configured").await;
    assert!(!missing.ok);
    assert!(missing.message.contains("not configured"));
}

#[tokio::test]
async fn disabled_servers_are_skipped_by_connect_all() {
    let server = MockMcp::start();
    let registry = McpServerRegistry::new(temp_dir("disabled"));
    let mut entry = http_entry("mock", &server.url);
    entry.enabled = false;
    registry.upsert(entry).unwrap();

    assert!(registry.connect_all().await.is_empty());
    let views = registry.list().servers;
    assert!(!views[0].enabled);
    assert!(!views[0].connected);
}
