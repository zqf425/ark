//! End-to-end tests for the local plugin host assembly.
//!
//! These drive [`assemble`] exactly as `wiring` does: a real `data_dir`, a real
//! `UiBus`, a real `McpBus`, and `plugin.json` files taken from
//! `apps/arkd/tests/fixtures/`. The point is that the composition root — not a
//! hand-built host — is what publishes tools and panels, so a wiring regression
//! fails here instead of in the running app.
//!
//! What is *not* re-tested here: the scan/plan/register rules themselves. Those
//! live in `ark-plugin` with their own suite; these tests only prove the
//! assembled result is reachable from the buses `arkd` actually hands out.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ark_mcp::McpBus;
use ark_ports::{SandboxDecision, Tool, ToolArgs, ToolContext};
use ark_ui::UiBus;

use super::*;

const MINIMAL: &str = include_str!("../../tests/fixtures/ark.fixture.minimal/plugin.json");
const EGRESS: &str = include_str!("../../tests/fixtures/ark.fixture.egress/plugin.json");

/// A data directory plus the buses `assemble` will populate.
struct Fixture {
    data_dir: tempfile::TempDir,
    ui: Arc<UiBus>,
    mcp: Arc<McpBus>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            data_dir: tempfile::tempdir().unwrap(),
            ui: Arc::new(UiBus::new()),
            mcp: Arc::new(McpBus::default()),
        }
    }

    /// Write one plugin folder under `<data_dir>/plugins/`.
    fn plugin(&self, body: &str) {
        let raw: serde_json::Value = serde_json::from_str(body).unwrap();
        let id = raw["id"].as_str().unwrap().to_string();
        self.plugin_with_id(&id, body);
    }

    /// Write a manifest whose folder name deliberately differs from its id, for
    /// the cases where the two need to disagree.
    fn plugin_with_id(&self, folder: &str, body: &str) {
        let directory = self.data_dir.path().join(PLUGINS_DIR).join(folder);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("plugin.json"), body).unwrap();
    }

    fn assemble(&self) -> PluginRuntime {
        assemble(self.data_dir.path(), &self.ui, Arc::clone(&self.mcp)).unwrap()
    }

    fn plugins_root(&self) -> PathBuf {
        self.data_dir.path().join(PLUGINS_DIR)
    }
}

fn tool_context() -> ToolContext {
    ToolContext {
        thread_id: ark_common::ThreadId::new(),
        turn_id: ark_common::TurnId::new(),
        workspace: PathBuf::from("."),
        sandbox: SandboxDecision::ReadOnly,
        env: Default::default(),
    }
}

/// The tool names the assembled runtime contributes to the `mcp` group.
fn tool_names(runtime: &PluginRuntime) -> Vec<String> {
    runtime
        .tools()
        .iter()
        .map(|tool| tool.name().to_string())
        .collect()
}

fn find_tool(runtime: &PluginRuntime, name: &str) -> Arc<dyn Tool> {
    runtime
        .tools()
        .into_iter()
        .find(|tool| tool.name() == name)
        .unwrap_or_else(|| panic!("{} is not published by the assembled host", name))
}

/// The happy path the requirement calls for: a directory plugin on disk becomes
/// a callable tool and a registered panel, with no arkd-specific glue per plugin.
#[tokio::test]
async fn a_directory_plugin_reaches_both_buses_through_the_composition_root() {
    let fixture = Fixture::new();
    fixture.plugin(MINIMAL);

    let runtime = fixture.assemble();
    assert_eq!(runtime.root(), fixture.plugins_root().as_path());
    assert_eq!(runtime.host().report().loaded, vec!["ark.fixture.minimal"]);
    assert!(runtime.host().report().is_healthy());

    // The tool is published under the bus namespace and answers through it.
    assert_eq!(tool_names(&runtime), vec!["ark.fixture.minimal.greet"]);
    let tool = find_tool(&runtime, "ark.fixture.minimal.greet");
    let output = tool
        .execute(&tool_context(), ToolArgs(serde_json::json!({})))
        .await
        .unwrap();
    assert!(output.ok, "{}", output.content);
    assert!(
        output.content.contains("hello from fixture"),
        "unexpected payload: {}",
        output.content
    );

    // The panel went to the same UI bus the shell reads.
    let panels = fixture.ui.panels();
    assert_eq!(panels.len(), 1);
    assert_eq!(panels[0].1.id.0, "ark.fixture.minimal.panel");

    // And the diagnostics route reports both contributions.
    let report = runtime.host().host_report();
    assert_eq!(report.plugins.len(), 1);
    assert_eq!(report.plugins[0].panels, vec!["ark.fixture.minimal.panel"]);
    assert_eq!(report.plugins[0].tools, vec!["ark.fixture.minimal.greet"]);
}

/// A broken file, a failing `register` and a dependency cycle must all degrade to
/// diagnostics, leaving the healthy plugin untouched.
#[test]
fn broken_manifests_and_cycles_cost_only_the_plugins_that_broke() {
    let fixture = Fixture::new();
    fixture.plugin(MINIMAL);

    // Unparseable manifest.
    fixture.plugin_with_id("ark.fixture.broken", "{ not json");

    // Syntactically fine, but `mcp.expose` promises a tool it never declares.
    let mut failing: serde_json::Value = serde_json::from_str(MINIMAL).unwrap();
    failing["id"] = serde_json::json!("ark.fixture.failing");
    failing["ui"]["panels"][0]["id"] = serde_json::json!("ark.fixture.failing.panel");
    failing["mcp"] = serde_json::json!({ "expose": ["greet", "absent"] });
    fixture.plugin_with_id(
        "ark.fixture.failing",
        &serde_json::to_string(&failing).unwrap(),
    );

    // Two plugins that require each other.
    for (id, other) in [
        ("ark.fixture.cycle_a", "ark.fixture.cycle_b"),
        ("ark.fixture.cycle_b", "ark.fixture.cycle_a"),
    ] {
        let mut body: serde_json::Value = serde_json::from_str(MINIMAL).unwrap();
        body["id"] = serde_json::json!(id);
        body["ui"]["panels"][0]["id"] = serde_json::json!(format!("{id}.panel"));
        body["requires"] = serde_json::json!({ other: "^1" });
        fixture.plugin_with_id(id, &serde_json::to_string(&body).unwrap());
    }

    let runtime = fixture.assemble();
    let report = runtime.host().report();
    assert_eq!(report.loaded, vec!["ark.fixture.minimal"]);
    assert_eq!(report.failed, vec!["ark.fixture.failing"]);
    // A cycle is dropped before `register` runs, so neither member appears in
    // `skipped`; each is explained by its own diagnostic instead.
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);

    // Only the healthy plugin contributes, to either bus.
    assert_eq!(tool_names(&runtime), vec!["ark.fixture.minimal.greet"]);
    assert_eq!(fixture.ui.panels().len(), 1);

    // Every drop is explainable from the diagnostics the route exposes.
    let kinds = |kind: &str| -> Vec<String> {
        runtime
            .host()
            .host_report()
            .diagnostics
            .into_iter()
            .filter(|d| d.kind == kind)
            .map(|d| d.plugin_id)
            .collect()
    };
    assert_eq!(kinds("plugin_manifest_invalid"), vec!["ark.fixture.broken"]);
    assert_eq!(kinds("plugin_load_failed"), vec!["ark.fixture.failing"]);
    assert_eq!(kinds("plugin_dependency_cycle").len(), 2);
}

/// Unloading has to withdraw both contributions, not just the plugin record.
#[tokio::test]
async fn unloading_a_plugin_leaves_no_tool_and_no_panel() {
    let fixture = Fixture::new();
    fixture.plugin(MINIMAL);

    let runtime = fixture.assemble();
    assert_eq!(tool_names(&runtime), vec!["ark.fixture.minimal.greet"]);
    assert_eq!(fixture.ui.panels().len(), 1);

    assert!(runtime.host().unload("ark.fixture.minimal"));

    assert!(
        runtime.tools().is_empty(),
        "an unloaded plugin must not keep a model-visible tool"
    );
    assert!(
        fixture.ui.panels().is_empty(),
        "an unloaded plugin must not keep a panel"
    );
    assert!(runtime.host().host_report().plugins.is_empty());
    assert!(!runtime.host().unload("ark.fixture.minimal"));
}

/// The egress fixture declares `api.example.com` but calls `evil.example.com`:
/// the call must fail and the refusal must be visible on the diagnostics route.
#[tokio::test]
async fn an_unauthorized_host_is_refused_and_audited() {
    let fixture = Fixture::new();
    fixture.plugin(EGRESS);

    let runtime = fixture.assemble();
    assert!(runtime.host().report().is_healthy());
    assert_eq!(tool_names(&runtime), vec!["ark.fixture.egress.fetch"]);

    let tool = find_tool(&runtime, "ark.fixture.egress.fetch");
    let output = tool
        .execute(&tool_context(), ToolArgs(serde_json::json!({})))
        .await
        .unwrap();
    assert!(!output.ok, "a host outside the allowlist must not answer");
    assert_eq!(
        output.detail.as_deref(),
        Some("host_not_allowed"),
        "the refusal must name its reason: {}",
        output.content
    );

    let report = runtime.host().host_report();
    assert_eq!(report.http_denials.len(), 1);
    assert_eq!(report.http_denials[0].plugin_id, "ark.fixture.egress");
    assert_eq!(report.http_denials[0].kind, "http_denied");
    assert!(
        report.http_denials[0].detail.contains("host_not_allowed"),
        "{:?}",
        report.http_denials
    );
    assert!(
        report.http_denials[0].detail.contains("evil.example.com"),
        "the diagnostic must name the refused host: {:?}",
        report.http_denials
    );
}

/// A missing plugin directory is not a startup failure: a fresh install has to
/// boot. `assemble` creates the directory so a later drop-in needs no restart of
/// anything but the process.
#[test]
fn an_absent_plugin_directory_still_boots() {
    let fixture = Fixture::new();
    assert!(!fixture.plugins_root().exists());

    let runtime = fixture.assemble();
    assert!(runtime.host().report().is_healthy());
    assert!(runtime.tools().is_empty());
    assert!(fixture.plugins_root().is_dir());
}

/// The report is what `GET /v1/runtime/plugins` serializes, so a plugin entry
/// must survive the wire round-trip with its qualified tool names intact.
#[test]
fn the_host_report_round_trips_through_json() {
    let fixture = Fixture::new();
    fixture.plugin(MINIMAL);
    let runtime = fixture.assemble();

    let json = serde_json::to_value(runtime.host().host_report()).unwrap();
    assert_eq!(json["plugins"][0]["id"], "ark.fixture.minimal");
    assert_eq!(json["plugins"][0]["tools"][0], "ark.fixture.minimal.greet");
    assert_eq!(json["plugins"][0]["capabilities"][0], "tool");
    assert!(json["http_denials"].is_array());
}

/// Guards the fixture files themselves: if one stops parsing, the end-to-end
/// tests above would fail for the wrong reason.
#[test]
fn the_fixtures_are_valid_manifests() {
    for body in [MINIMAL, EGRESS] {
        let raw: serde_json::Value = serde_json::from_str(body).unwrap();
        assert!(raw["id"].as_str().is_some(), "fixture needs an id");
        assert!(raw["version"].as_str().is_some(), "fixture needs a version");
    }
    assert_ne!(
        Path::new("ark.fixture.minimal"),
        Path::new("ark.fixture.egress")
    );
}
