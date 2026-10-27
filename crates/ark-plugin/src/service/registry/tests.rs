//! Loader tests: topology order, fail-closed skipping, Drop unregistration.

use super::*;
use crate::domain::PluginKind;
use crate::service::plugin::Plugin;
use ark_contract::{PageId, PageSpec, Placement};
use ark_ports::UiCapability;
use ark_ui::UiBus;

/// Log sink that only records, so no test depends on a real logger.
#[derive(Default)]
struct NullLog;

impl ark_ports::LogService for NullLog {
    fn log(&self, _record: ark_ports::services::LogRecord) {}
}

#[derive(Default)]
struct NullTrace;

impl ark_ports::TraceService for NullTrace {
    fn span(&self, _name: &str, _fields: BTreeMap<String, String>) {}
}

#[derive(Default)]
struct NullMetrics;

impl ark_ports::MetricService for NullMetrics {
    fn counter(&self, _name: &str, _value: u64) {}
}

#[derive(Default)]
struct NullNotify;

impl ark_ports::NotifyService for NullNotify {
    fn notify(&self, _title: &str, _body: &str) -> Result<()> {
        Ok(())
    }
}

fn handles() -> HostHandles {
    HostHandles {
        http: Arc::new(
            ark_http::HttpBus::new(
                ark_http::HttpPolicy::default(),
                Arc::new(ark_http::CredentialVault::new()),
                Arc::new(ark_http::MemoryDiagnostics::new(16)),
            )
            .unwrap(),
        ),
        mcp: Arc::new(ark_mcp::McpBus::default()),
        ui: Arc::new(UiBus::new()),
        ui_bus: Some(Arc::new(UiBus::new())),
        mcp_bus: Some(Arc::new(ark_mcp::McpBus::default())),
        vault: Arc::new(ark_http::CredentialVault::new()),
        log: Arc::new(NullLog),
        cfg_store: Arc::new(Mutex::new(BTreeMap::new())),
        kv_store: Arc::new(Mutex::new(BTreeMap::new())),
        plugins_root: std::env::temp_dir().join("ark-plugin-registry-tests"),
        trace: Arc::new(NullTrace),
        metrics: Arc::new(NullMetrics),
        notify: Arc::new(NullNotify),
        events: None,
    }
}

fn manifests() -> Vec<PluginManifest> {
    Vec::new()
}

/// A plugin that registers one page, proving `Drop` unregistration end to end.
struct PagePlugin {
    manifest: PluginManifest,
    page_id: String,
}

impl Plugin for PagePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn register(&self, ctx: &mut PluginContext) -> Result<Registration> {
        let page = PageSpec {
            id: PageId::new(&self.page_id),
            route: format!("/{}", self.page_id.replace('.', "-")),
            title: "Test".into(),
            icon: None,
            placement: Placement::PrimaryNav,
            bundle: None,
            required_caps: vec![],
        };
        let mut registration = Registration::new();
        registration.push_page(ctx.register_page(page)?);
        Ok(registration)
    }
}

/// A plugin whose `register` always fails.
struct BrokenPlugin {
    manifest: PluginManifest,
}

impl Plugin for BrokenPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn register(&self, _ctx: &mut PluginContext) -> Result<Registration> {
        Err(ArkError::internal("boom").with_detail("test_failure"))
    }
}

fn page_manifest(id: &str, page_id: &str) -> PluginManifest {
    let mut manifest = PluginManifest::new(id, "1.0.0", PluginKind::UiPage).unwrap();
    manifest.capabilities = vec![ark_contract::Capability::UiPage];
    let _ = page_id;
    manifest
}

#[test]
fn loading_registers_pages_and_unload_removes_them() {
    let ui = Arc::new(UiBus::new());
    let mut host = handles();
    host.ui_bus = Some(ui.clone());
    host.ui = ui.clone();

    let plugin: Arc<dyn Plugin> = Arc::new(PagePlugin {
        manifest: page_manifest("ark.test.page", "ark.test.page.main"),
        page_id: "ark.test.page.main".to_string(),
    });

    let diagnostics = Arc::new(MemoryPluginDiagnostics::new());
    let mut registry = PluginRegistry::new(host, diagnostics.clone());
    let report = registry.load(&[plugin]).unwrap();

    assert_eq!(report.loaded, vec!["ark.test.page"]);
    assert!(report.is_healthy());
    assert_eq!(ui.pages().len(), 1, "page must be registered on the bus");

    assert!(registry.unload("ark.test.page"));
    assert!(ui.pages().is_empty(), "Drop must unregister the page");
}

#[test]
fn a_failing_plugin_is_skipped_with_a_diagnostic() {
    let diagnostics = Arc::new(MemoryPluginDiagnostics::new());
    let mut registry = PluginRegistry::new(handles(), diagnostics.clone());
    let broken: Arc<dyn Plugin> = Arc::new(BrokenPlugin {
        manifest: page_manifest("ark.test.broken", "ark.test.broken.page"),
    });
    let report = registry.load(&[broken]).unwrap();

    assert_eq!(report.failed, vec!["ark.test.broken"]);
    assert_eq!(diagnostics.failed_count(), 1);
    assert!(registry.loaded_ids().is_empty());
}

#[test]
fn dependents_of_a_failed_plugin_are_skipped_not_started() {
    let mut dependent = page_manifest("ark.test.dependent", "ark.test.dependent.page");
    dependent
        .requires
        .insert("ark.test.broken".into(), "^1".into());

    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(BrokenPlugin {
            manifest: page_manifest("ark.test.broken", "ark.test.broken.page"),
        }),
        Arc::new(PagePlugin {
            manifest: dependent,
            page_id: "ark.test.dependent.page".into(),
        }),
    ];

    let diagnostics = Arc::new(MemoryPluginDiagnostics::new());
    let mut registry = PluginRegistry::new(handles(), diagnostics);
    let report = registry.load(&plugins).unwrap();

    assert_eq!(report.failed, vec!["ark.test.broken"]);
    assert_eq!(report.skipped, vec!["ark.test.dependent"]);
    assert!(registry.loaded_ids().is_empty());
}

#[test]
fn a_declaration_cycle_aborts_the_whole_load() {
    let mut a = page_manifest("ark.test.a", "ark.test.a.page");
    a.requires.insert("ark.test.b".into(), "^1".into());
    let mut b = page_manifest("ark.test.b", "ark.test.b.page");
    b.requires.insert("ark.test.a".into(), "^1".into());

    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(PagePlugin {
            manifest: a,
            page_id: "ark.test.a.page".into(),
        }),
        Arc::new(PagePlugin {
            manifest: b,
            page_id: "ark.test.b.page".into(),
        }),
    ];
    let mut registry = PluginRegistry::new(handles(), Arc::new(MemoryPluginDiagnostics::new()));
    let err = registry.load(&plugins).unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("plugin_dependency_cycle"));
    assert!(
        registry.loaded_ids().is_empty(),
        "nothing may be registered"
    );
}

#[test]
fn report_json_lists_contributed_capabilities() {
    let mut host = handles();
    let ui = Arc::new(UiBus::new());
    host.ui_bus = Some(ui);
    let plugin: Arc<dyn Plugin> = Arc::new(PagePlugin {
        manifest: page_manifest("ark.test.page", "ark.test.page.main"),
        page_id: "ark.test.page.main".into(),
    });
    let mut registry = PluginRegistry::new(host, Arc::new(MemoryPluginDiagnostics::new()));
    registry.load(&[plugin]).unwrap();

    let json = registry.report_json();
    assert_eq!(json["plugins"][0]["id"], "ark.test.page");
    assert_eq!(json["plugins"][0]["pages"][0], "ark.test.page.main");
    assert_eq!(json["plugins"][0]["kind"], "ui_page");
}

#[test]
fn empty_plugin_set_loads_cleanly() {
    let mut registry = PluginRegistry::new(handles(), Arc::new(MemoryPluginDiagnostics::new()));
    let report = registry.load(&[]).unwrap();
    assert!(report.is_healthy());
    assert!(manifests().is_empty());
}
