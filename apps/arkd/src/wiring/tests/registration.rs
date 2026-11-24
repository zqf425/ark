use super::*;

#[test]
fn request_user_input_is_registered_for_both_agent_tool_sets() {
    let workspace = tempfile::tempdir().expect("temp dir is creatable");
    let skills = Arc::new(ark_skills::SkillRuntime::fixed(
        ark_skills::SkillsConfig::default(),
        workspace.path().to_path_buf(),
    ));
    // An inert capability is enough: registration only reads the catalogue.
    let http: Arc<dyn ark_ports::HttpCapability> = Arc::new(InertHttp);
    let child_tools =
        build_child_tools(Arc::clone(&skills), Arc::clone(&http)).expect("child tools build");
    assert!(child_tools
        .names()
        .contains(&"request_user_input".to_string()));
    assert!(child_tools.names().contains(&"web_fetch".to_string()));

    // The parent set adds `delegate_task` and the durable plan tool on top of
    // the child set.
    let data_dir = tempfile::tempdir().expect("temp dir");
    let store: Arc<dyn SessionStore> = Arc::new(
        ark_store::FileSessionStore::open(ark_store::StoreConfig::new(data_dir.path()))
            .expect("session store"),
    );
    let parent_tools = build_tools(
        Arc::new(TestSubagentRunner),
        Arc::new(TestPolicy) as Arc<dyn ark_ports::SubagentPolicySource>,
        Arc::clone(&skills),
        http,
        Arc::clone(&store),
    )
    .expect("tools build");
    let names = parent_tools.names();
    assert!(names.contains(&"request_user_input".to_string()));
    assert!(names.contains(&"bash".to_string()));
    assert!(names.contains(&"delegate_task".to_string()));
    assert!(names.contains(&"web_fetch".to_string()));
    assert!(names.contains(&"todo_write".to_string()));

    // The durable plan tool is parent-only: the child set must not offer it.
    assert!(!child_tools.names().contains(&"todo_write".to_string()));

    // The wire schema fixes the prompt contract: {prompt: string, required}.
    let schema = parent_tools
        .schemas()
        .into_iter()
        .find(|schema| schema["name"] == "request_user_input")
        .expect("request_user_input schema present");
    assert_eq!(schema["parameters"]["required"][0], "prompt");
    assert_eq!(
        schema["parameters"]["properties"]["prompt"]["type"],
        "string"
    );
}

/// Registry-shape stub: `build_tools` only needs the runner for the
/// `delegate_task` tool; this test never runs a child.
struct TestSubagentRunner;

/// Policy stub: only the registration shape is under test here.
struct TestPolicy;

impl ark_ports::SubagentPolicySource for TestPolicy {
    fn policy(&self) -> ark_ports::SubagentPolicy {
        ark_ports::SubagentPolicy::default()
    }
}

/// Capability stub for registration tests; `web_fetch` is never executed here.
struct InertHttp;

#[async_trait::async_trait]
impl ark_ports::HttpCapability for InertHttp {
    async fn request(
        &self,
        _plugin_id: &str,
        _req: ark_ports::OutboundRequest,
    ) -> ark_common::Result<ark_ports::OutboundResponse> {
        Err(ark_common::ArkError::internal("not used in tests"))
    }
}

#[async_trait::async_trait]
impl ark_ports::SubagentRunner for TestSubagentRunner {
    fn is_enabled(&self) -> bool {
        true
    }

    fn active_count(&self) -> u32 {
        0
    }

    async fn delegate(
        &self,
        _request: ark_ports::SubagentRequest,
    ) -> ark_common::Result<ark_ports::SubagentOutcome> {
        Err(ark_common::ArkError::internal("not used by this test"))
    }

    async fn delegate_background(
        &self,
        _request: ark_ports::SubagentRequest,
    ) -> ark_common::Result<ark_common::ChildId> {
        Err(ark_common::ArkError::internal("not used by this test"))
    }

    async fn prepare_child(
        &self,
        _request: ark_ports::SubagentRequest,
    ) -> ark_common::Result<ark_common::ChildId> {
        Err(ark_common::ArkError::internal("not used by this test"))
    }

    async fn run_child(
        &self,
        _child: ark_common::ChildId,
    ) -> ark_common::Result<ark_ports::SubagentOutcome> {
        Err(ark_common::ArkError::internal("not used by this test"))
    }
}

/// The invariant the headless fix rests on: `wiring::build` hands out the
/// schedule handle but never starts the loop — only `serve` does. If a future
/// refactor moves `spawn` back into `build`, a headless `run` would silently
/// fire due schedules again.
#[tokio::test]
async fn build_hands_out_the_schedule_handle_without_starting_the_loop() {
    let data_dir = TestDataDir::new();
    let workspace = tempfile::tempdir().expect("temp dir is creatable");
    let args = crate::cli::ServeArgs {
        port: 0,
        data_dir: data_dir.path().to_path_buf(),
        auth_token: None,
        workspace: workspace.path().to_path_buf(),
    };
    let wiring = crate::wiring::build(&args, false)
        .await
        .expect("runtime build");
    assert!(!wiring.schedules.is_started());
}

#[test]
fn skills_config_source_reads_the_latest_snapshot() {
    // The composition root injects this closure into `SkillRuntime`; a saved
    // skills patch must be visible through it without rebuilding the runtime
    // (skills is a `Hot` reload class).
    let store = Arc::new(ConfigStore::new(RuntimeConfig::default()));
    let global_skills = std::path::PathBuf::from("/tmp/global-skills");
    let source = skills_config_source(store.clone(), global_skills.clone());
    assert!(source().enabled);
    // First root is the app-owned global dir; the shared `~/.agents/skills`
    // convention follows (resolved from the real home, never `/tmp/...`).
    assert_eq!(source().roots[0], global_skills);
    assert_eq!(source().roots[1], ark_skills::shared_skills_root());
    assert!(source().roots[1].ends_with(".agents/skills"));

    let applied = store.apply(ark_contract::ConfigPatch {
        skills: Some(ark_contract::SkillsSettings {
            enabled: false,
            extra_dirs: vec!["/tmp/extra-skills".into()],
        }),
        ..ark_contract::ConfigPatch::default()
    });
    assert!(
        matches!(applied, ark_contract::ApplyResult::Applied { .. }),
        "skills patch applies hot: {applied:?}"
    );

    let config = source();
    assert!(!config.enabled);
    assert!(config
        .roots
        .contains(&std::path::PathBuf::from("/tmp/extra-skills")));
}
