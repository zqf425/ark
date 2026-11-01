//! Host loader tests: the four acceptance scenarios of the wiring requirement.
//!
//! Each test drives a real `plugin.json` on disk through the real loader, so the
//! scan -> plan -> load path is exercised instead of a hand-built plugin object.

use super::*;
use crate::service::directory::MANIFEST_FILE;
use ark_ports::services::LogRecord;
use ark_ports::{LogService, MetricService, NotifyService, TraceService, UiCapability};
use ark_ui::UiBus;
use std::collections::BTreeMap;

#[derive(Default)]
struct NullLog;

impl LogService for NullLog {
    fn log(&self, _record: LogRecord) {}
}

#[derive(Default)]
struct NullTrace;

impl TraceService for NullTrace {
    fn span(&self, _name: &str, _fields: BTreeMap<String, String>) {}
}

#[derive(Default)]
struct NullMetrics;

impl MetricService for NullMetrics {
    fn counter(&self, _name: &str, _value: u64) {}
}

#[derive(Default)]
struct NullNotify;

impl NotifyService for NullNotify {
    fn notify(&self, _title: &str, _body: &str) -> ark_common::Result<()> {
        Ok(())
    }
}

/// A host bundle whose UI and MCP buses the test can inspect afterwards.
struct Fixture {
    root: tempfile::TempDir,
    ui: Arc<UiBus>,
    mcp: Arc<McpBus>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            ui: Arc::new(UiBus::new()),
            mcp: Arc::new(McpBus::default()),
        }
    }

    /// The bus plus the same diagnostics sink it records into.
    fn http(&self) -> (Arc<HttpBus>, Arc<MemoryDiagnostics>) {
        let sink = Arc::new(MemoryDiagnostics::new(64));
        let bus = Arc::new(
            HttpBus::new(
                ark_http::HttpPolicy::default(),
                Arc::new(ark_http::CredentialVault::new()),
                sink.clone(),
            )
            .unwrap(),
        );
        (bus, sink)
    }

    fn handles(&self, http: Arc<HttpBus>) -> HostHandles {
        HostHandles {
            http: Arc::clone(&http) as Arc<dyn ark_ports::HttpCapability>,
            mcp: Arc::clone(&self.mcp) as Arc<dyn ark_ports::McpCapability>,
            ui: Arc::clone(&self.ui) as Arc<dyn ark_ports::UiCapability>,
            ui_bus: Some(Arc::clone(&self.ui)),
            mcp_bus: Some(Arc::clone(&self.mcp)),
            vault: Arc::new(ark_http::CredentialVault::new()),
            log: Arc::new(NullLog),
            cfg_store: Arc::new(Mutex::new(BTreeMap::new())),
            kv_store: Arc::new(Mutex::new(BTreeMap::new())),
            plugins_root: self.root.path().to_path_buf(),
            trace: Arc::new(NullTrace),
            metrics: Arc::new(NullMetrics),
            notify: Arc::new(NullNotify),
            events: None,
        }
    }

    fn write(&self, name: &str, body: serde_json::Value) {
        let directory = self.root.path().join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    fn load(&self) -> (PluginHost, Arc<MemoryDiagnostics>) {
        let (http, sink) = self.http();
        let host = load_host(
            self.root.path(),
            self.handles(Arc::clone(&http)),
            http,
            sink.clone(),
        )
        .unwrap();
        (host.with_mcp_bus(Arc::clone(&self.mcp)), sink)
    }
}

fn hello_plugin() -> serde_json::Value {
    serde_json::json!({
        "id": "ark.example.hello",
        "version": "1.0.0",
        "kind": "tool",
        "capabilities": ["tool", "ui_panel"],
        "mcp": { "expose": ["greet"] },
        "ui": {
            "panels": [{ "id": "ark.example.hello.panel", "slot": "right", "title": "Hello" }]
        },
        "tools": [{
            "name": "greet",
            "description": "says hello",
            "action": { "kind": "static", "content": { "text": "hi" } }
        }]
    })
}

#[test]
fn a_minimal_plugin_becomes_visible_on_both_buses() {
    let fixture = Fixture::new();
    fixture.write("ark.example.hello", hello_plugin());
    let (host, _) = fixture.load();

    assert_eq!(host.report().loaded, vec!["ark.example.hello"]);
    assert!(host.report().is_healthy());
    assert_eq!(fixture.ui.pages().len(), 0, "no page was declared");
    assert_eq!(
        fixture.mcp.catalog().tools.len(),
        1,
        "the declared tool must reach the MCP bus"
    );
    assert_eq!(
        fixture.mcp.catalog().tools[0].plugin_id,
        "ark.example.hello"
    );

    let report = host.host_report();
    let entry = &report.plugins[0];
    assert_eq!(entry.id, "ark.example.hello");
    assert_eq!(entry.panels, vec!["ark.example.hello.panel".to_string()]);
    assert_eq!(entry.tools, vec!["ark.example.hello.greet".to_string()]);
    assert!(report.diagnostics.iter().any(|d| d.kind == "plugin_loaded"));
}

#[test]
fn a_declared_tool_answers_through_the_bus() {
    let fixture = Fixture::new();
    fixture.write("ark.example.hello", hello_plugin());
    let (host, _) = fixture.load();
    assert!(host.report().is_healthy());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime
        .block_on(fixture.mcp.dispatch(
            "ark.example.hello",
            "ark.example.hello",
            "greet",
            serde_json::json!({}),
        ))
        .unwrap();
    assert!(result.ok);
    assert_eq!(result.content["text"], "hi");
}

#[test]
fn a_broken_manifest_is_a_diagnostic_and_healthy_plugins_still_load() {
    let fixture = Fixture::new();
    fixture.write("ark.example.hello", hello_plugin());
    let broken = fixture.root.path().join("ark.example.broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join(MANIFEST_FILE), b"{ not json").unwrap();

    let (host, _) = fixture.load();
    assert_eq!(host.report().loaded, vec!["ark.example.hello"]);
    let diagnostics = host.diagnostics();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.kind == "plugin_manifest_invalid" && d.plugin_id == "ark.example.broken"),
        "a broken file must explain itself: {diagnostics:?}"
    );
    assert_eq!(fixture.mcp.catalog().tools.len(), 1);
}

#[test]
fn a_register_failure_skips_only_that_plugin() {
    let fixture = Fixture::new();
    fixture.write("ark.example.hello", hello_plugin());
    // `mcp.expose` promises a tool the file never declares, so `register` fails.
    let mut broken = hello_plugin();
    broken["id"] = serde_json::json!("ark.example.broken");
    broken["ui"]["panels"][0]["id"] = serde_json::json!("ark.example.broken.panel");
    broken["mcp"] = serde_json::json!({ "expose": ["greet", "absent"] });
    fixture.write("ark.example.broken", broken);

    let (host, _) = fixture.load();
    assert_eq!(host.report().loaded, vec!["ark.example.hello"]);
    assert_eq!(host.report().failed, vec!["ark.example.broken"]);
    assert!(host
        .diagnostics()
        .iter()
        .any(|d| d.kind == "plugin_load_failed" && d.plugin_id == "ark.example.broken"));
}

#[test]
fn a_dependency_cycle_is_dropped_without_stopping_the_runtime() {
    let fixture = Fixture::new();
    fixture.write("ark.example.hello", hello_plugin());
    for (id, other) in [
        ("ark.example.a", "ark.example.b"),
        ("ark.example.b", "ark.example.a"),
    ] {
        let mut body = hello_plugin();
        body["id"] = serde_json::json!(id);
        body["ui"]["panels"][0]["id"] = serde_json::json!(format!("{id}.panel"));
        body["requires"] = serde_json::json!({ other: "^1" });
        fixture.write(id, body);
    }

    let (host, _) = fixture.load();
    assert_eq!(host.report().loaded, vec!["ark.example.hello"]);
    let codes: Vec<String> = host
        .diagnostics()
        .into_iter()
        .filter(|d| d.kind == "plugin_dependency_cycle")
        .map(|d| d.plugin_id)
        .collect();
    assert_eq!(codes, vec!["ark.example.a", "ark.example.b"]);
}

#[test]
fn unloading_a_plugin_removes_its_tools_and_panels() {
    let fixture = Fixture::new();
    fixture.write("ark.example.hello", hello_plugin());
    let (host, _) = fixture.load();
    assert_eq!(fixture.mcp.catalog().tools.len(), 1);

    assert!(host.unload("ark.example.hello"));
    assert!(
        fixture.mcp.catalog().tools.is_empty(),
        "an unloaded plugin must leave no tool behind"
    );
    assert!(host.host_report().plugins.is_empty());
    assert!(!host.unload("ark.example.hello"), "unload is idempotent");
}

#[test]
fn a_request_to_an_undeclared_host_is_denied_and_audited() {
    let fixture = Fixture::new();
    let mut body = hello_plugin();
    body["http"] = serde_json::json!({ "allow": ["api.example.com"] });
    body["mcp"] = serde_json::json!({ "expose": ["fetch"] });
    body["tools"] = serde_json::json!([{
        "name": "fetch",
        "action": { "kind": "http", "url": "https://evil.example.com/steal" }
    }]);
    fixture.write("ark.example.hello", body);

    let (host, sink) = fixture.load();
    assert!(host.report().is_healthy());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(fixture.mcp.dispatch(
            "ark.example.hello",
            "ark.example.hello",
            "fetch",
            serde_json::json!({}),
        ))
        .unwrap_err();
    assert_eq!(error.detail.as_deref(), Some("host_not_allowed"));

    let denials = host.host_report().http_denials;
    assert_eq!(denials.len(), 1);
    assert_eq!(denials[0].plugin_id, "ark.example.hello");
    assert!(
        denials[0].detail.contains("host_not_allowed"),
        "{denials:?}"
    );
    assert_eq!(
        sink.records().len(),
        1,
        "the denial must be recorded exactly once"
    );
}

#[test]
fn a_failed_plugin_keeps_no_egress_rights() {
    let fixture = Fixture::new();
    let mut broken = hello_plugin();
    broken["id"] = serde_json::json!("ark.example.broken");
    broken["ui"]["panels"][0]["id"] = serde_json::json!("ark.example.broken.panel");
    broken["http"] = serde_json::json!({ "allow": ["api.example.com"] });
    broken["mcp"] = serde_json::json!({ "expose": ["greet", "absent"] });
    fixture.write("ark.example.broken", broken);

    let (http, sink) = fixture.http();
    let host = load_host(
        fixture.root.path(),
        fixture.handles(Arc::clone(&http)),
        Arc::clone(&http),
        sink,
    )
    .unwrap();
    assert_eq!(host.report().failed, vec!["ark.example.broken"]);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(ark_ports::HttpCapability::request(
            http.as_ref(),
            "ark.example.broken",
            ark_ports::OutboundRequest {
                method: "GET".into(),
                url: "https://api.example.com/v1".into(),
                ..ark_ports::OutboundRequest::default()
            },
        ))
        .unwrap_err();
    assert_eq!(
        error.detail.as_deref(),
        Some("empty_allowlist"),
        "a plugin that never started must not keep egress"
    );
}

#[test]
fn an_empty_directory_loads_cleanly() {
    let fixture = Fixture::new();
    let (host, _) = fixture.load();
    assert!(host.report().is_healthy());
    assert!(host.host_report().plugins.is_empty());
}
