//! Integration tests: the plugin loaded through the host's public registry path.
//!
//! `PluginRegistry::new` + `load` is exactly what `arkd`'s composition root
//! calls, and it is the only way an out-of-crate plugin can obtain a
//! `PluginContext` (`PluginContext::host_build` and `PluginIdentity::host_issue`
//! are `pub(crate)` on purpose). Driving the tests through it therefore also
//! pins that our plugin works in the position the host actually puts it in.

use std::sync::Arc;

use ark_mcp::McpBus;
use ark_plugin::service::{HostHandles, MemoryPluginDiagnostics, PluginRegistry};
use ark_plugin::Plugin;
use ark_ports::McpCapability;
use ark_ui::UiBus;
use serde_json::{json, Value};

use crate::{MemoryPlugin, PLUGIN_ID};

/// A host assembly over a fresh data directory, plus the bus our tools land on.
struct Host {
    registry: PluginRegistry,
    bus: Arc<McpBus>,
    _dir: tempfile::TempDir,
}

fn host() -> Host {
    host_with_root(None)
}

/// Build the host over a plugin root. `root: None` uses a fresh directory;
/// `Some` lets two host assemblies share one plugin root, which is how the
/// restart test proves persistence comes from files and not from memory.
fn host_with_root(root: Option<std::path::PathBuf>) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let root = root.unwrap_or_else(|| dir.path().to_path_buf());
    let bus = Arc::new(McpBus::default());
    let handles = HostHandles {
        http: Arc::new(NoHttp) as Arc<dyn ark_ports::HttpCapability>,
        mcp: Arc::clone(&bus) as Arc<dyn ark_ports::McpCapability>,
        ui: Arc::new(UiBus::new()) as Arc<dyn ark_ports::UiCapability>,
        ui_bus: None,
        mcp_bus: Some(Arc::clone(&bus)),
        vault: Arc::new(ark_http::CredentialVault::new()),
        log: Arc::new(NoLog),
        cfg_store: Arc::new(std::sync::Mutex::new(Default::default())),
        kv_store: Arc::new(std::sync::Mutex::new(Default::default())),
        plugins_root: root,
        trace: Arc::new(NoTrace),
        metrics: Arc::new(NoMetrics),
        notify: Arc::new(NoNotify),
        events: None,
    };
    let registry = PluginRegistry::new(handles, Arc::new(MemoryPluginDiagnostics::new()));
    Host {
        registry,
        bus,
        _dir: dir,
    }
}

impl Host {
    /// Load the memory plugin the way the composition root does.
    fn load(&mut self) -> MemoryPlugin {
        let plugin = MemoryPlugin::from_manifest_json(MemoryPlugin::MANIFEST_JSON).unwrap();
        let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(plugin)];
        let report = self.registry.load(&plugins).unwrap();
        assert_eq!(report.loaded, vec![PLUGIN_ID.to_string()], "{report:?}");
        assert!(report.is_healthy(), "{report:?}");
        MemoryPlugin::from_manifest_json(MemoryPlugin::MANIFEST_JSON).unwrap()
    }

    /// Call one published tool through the bus, as the model path does.
    async fn call(&self, tool: &str, args: Value) -> Value {
        let result = self
            .bus
            .call(PLUGIN_ID, PLUGIN_ID, tool, args)
            .await
            .unwrap();
        assert!(result.ok, "{tool} refused: {}", result.content);
        result.content
    }

    /// Call a tool expecting a structured refusal.
    async fn refuse(&self, tool: &str, args: Value) -> Value {
        let result = self
            .bus
            .call(PLUGIN_ID, PLUGIN_ID, tool, args)
            .await
            .unwrap();
        assert!(!result.ok, "{tool} should have refused: {}", result.content);
        result.content
    }
}

#[tokio::test]
async fn loading_publishes_all_four_tools() {
    let mut host = host();
    host.load();

    // The catalog is keyed, so the order is the bus's (sorted), not ours.
    let mut names: Vec<String> = host
        .bus
        .catalog()
        .tools
        .into_iter()
        .map(|spec| spec.tool)
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "memory_delete",
            "memory_list",
            "memory_search",
            "memory_store"
        ]
    );
}

#[tokio::test]
async fn a_stored_memory_is_found_by_search_and_by_list() {
    let mut host = host();
    host.load();

    let stored = host
        .call(
            "memory_store",
            json!({ "text": "The user writes in Simplified Chinese", "tags": ["language"] }),
        )
        .await;
    let id = stored["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("mem_"), "{id}");

    let found = host
        .call("memory_search", json!({ "query": "chinese" }))
        .await;
    assert_eq!(found["count"], 1);
    assert_eq!(found["memories"][0]["id"], id.as_str());

    let listed = host.call("memory_list", json!({})).await;
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["memories"][0]["tags"][0], "language");
}

#[tokio::test]
async fn tag_filters_exclude_non_matching_memories() {
    let mut host = host();
    host.load();
    host.call(
        "memory_store",
        json!({ "text": "likes tabs", "tags": ["style"] }),
    )
    .await;
    host.call(
        "memory_store",
        json!({ "text": "writes Chinese", "tags": ["language"] }),
    )
    .await;

    let hits = host
        .call(
            "memory_search",
            json!({ "query": "", "tags": ["language"] }),
        )
        .await;
    assert_eq!(hits["count"], 1);
    assert_eq!(hits["memories"][0]["text"], "writes Chinese");
}

#[tokio::test]
async fn delete_removes_the_memory_from_every_read_path() {
    let mut host = host();
    host.load();
    let stored = host
        .call("memory_store", json!({ "text": "temporary" }))
        .await;
    let id = stored["id"].as_str().unwrap().to_string();

    host.call("memory_delete", json!({ "id": id })).await;

    assert_eq!(host.call("memory_list", json!({})).await["total"], 0);
    assert_eq!(
        host.call("memory_search", json!({ "query": "temporary" }))
            .await["count"],
        0
    );
    // A second delete is a structured refusal, not a silent success.
    host.refuse("memory_delete", json!({ "id": id })).await;
}

#[tokio::test]
async fn empty_text_is_refused_without_writing_anything() {
    let mut host = host();
    host.load();
    host.refuse("memory_store", json!({ "text": "   " })).await;
    assert_eq!(host.call("memory_list", json!({})).await["total"], 0);
}

#[tokio::test]
async fn memories_survive_a_restart_over_the_same_plugin_root() {
    // One directory, two independent host assemblies over it: the second has a
    // fresh KV map (it is memory-only) so the file root is the only thing that
    // can carry the memory across.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    {
        let mut first = host_with_root(Some(root.clone()));
        first.load();
        first
            .call("memory_store", json!({ "text": "durable fact" }))
            .await;
    }

    let mut second = host_with_root(Some(root));
    second.load();
    let listed = second.call("memory_list", json!({})).await;
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["memories"][0]["text"], "durable fact");
}

#[test]
fn the_manifest_shipped_with_the_crate_is_valid_and_matches_the_id() {
    let manifest = MemoryPlugin::from_manifest_json(MemoryPlugin::MANIFEST_JSON).unwrap();
    assert_eq!(manifest.manifest().id, PLUGIN_ID);
    assert_eq!(manifest.manifest().mcp.expose.len(), 4);
}

// ---- minimal platform fakes: this plugin uses none of these ----

struct NoHttp;
#[async_trait::async_trait]
impl ark_ports::HttpCapability for NoHttp {
    async fn request(
        &self,
        _plugin_id: &str,
        _req: ark_ports::OutboundRequest,
    ) -> ark_common::Result<ark_ports::OutboundResponse> {
        Err(ark_common::ArkError::internal("no http in this test"))
    }
}

struct NoLog;
impl ark_ports::LogService for NoLog {
    fn log(&self, _record: ark_ports::services::LogRecord) {}
}

struct NoTrace;
impl ark_ports::TraceService for NoTrace {
    fn span(&self, _name: &str, _fields: std::collections::BTreeMap<String, String>) {}
}

struct NoMetrics;
impl ark_ports::MetricService for NoMetrics {
    fn counter(&self, _name: &str, _value: u64) {}
}

struct NoNotify;
impl ark_ports::NotifyService for NoNotify {
    fn notify(&self, _title: &str, _body: &str) -> ark_common::Result<()> {
        Ok(())
    }
}
