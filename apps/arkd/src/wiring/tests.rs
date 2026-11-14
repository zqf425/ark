//! Wiring tests: startup config, turn lifecycle, history replay and tool sets.

use super::history::conversation_history;
use super::tools::{build_child_tools, build_tools};
use super::turns::{
    abort_interrupted_turns, completion_inbox, finish_parent_turn, user_message_with_attachments,
};
use super::*;

/// Collects formatted log lines so the startup test can assert on the real
/// warning instead of only on the data behind it.
#[derive(Clone, Default)]
struct CaptureWriter(Arc<std::sync::Mutex<Vec<u8>>>);

impl CaptureWriter {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("capture lock")).into_owned()
    }
}

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("capture lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Run `body` under a scoped subscriber: the process-global subscriber is
/// left alone, so parallel tests never race on tracing installation.
fn capture_startup_logs<T>(body: impl FnOnce() -> T) -> (T, String) {
    let writer = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .finish();
    let value = tracing::subscriber::with_default(subscriber, body);
    (value, writer.text())
}

/// Defect D2: the GUI writes `serve`/`models`/`runtime`, so a copied file
/// used to start the runtime on pure defaults without a single log line.
#[test]
fn gui_shaped_config_warns_at_startup_and_keeps_defaults() {
    let data_dir = TestDataDir::new();
    std::fs::write(
        ark_config::config_path(data_dir.path()),
        br#"{"serve":{"port":18801},"models":[],"runtime":{"logLevel":"debug"}}"#,
    )
    .expect("config is writable");

    let (loaded, logs) = capture_startup_logs(|| load_startup_config(data_dir.path()));
    let loaded = loaded.expect("a parseable document still loads");

    assert_eq!(loaded.subagents.max_concurrency, 2);
    assert!(!loaded.subagents.auto_continue);
    assert!(logs.contains("WARN"), "startup must warn: {logs}");
    assert!(logs.contains("defaulted=[port"), "{logs}");
    assert!(logs.contains("ignored=[models"), "{logs}");
}

#[test]
fn matching_config_starts_without_a_warning() {
    let data_dir = TestDataDir::new();
    let path = ark_config::config_path(data_dir.path());
    let mut config = RuntimeConfig::default();
    config.subagents.max_concurrency = 5;
    config.subagents.auto_continue = true;
    ark_config::save(&path, &config).expect("config is writable");

    let (loaded, logs) = capture_startup_logs(|| load_startup_config(data_dir.path()));

    assert_eq!(loaded.expect("config loads"), config);
    assert!(logs.is_empty(), "a matching file must stay silent: {logs}");
}

#[test]
fn missing_config_is_a_silent_first_run() {
    let data_dir = TestDataDir::new();
    let (loaded, logs) = capture_startup_logs(|| load_startup_config(data_dir.path()));
    assert_eq!(loaded.expect("defaults load"), RuntimeConfig::default());
    assert!(logs.is_empty(), "a first run must stay silent: {logs}");
}

#[test]
fn unparseable_config_still_fails_startup() {
    let data_dir = TestDataDir::new();
    std::fs::write(ark_config::config_path(data_dir.path()), b"{ not json")
        .expect("config is writable");
    assert!(load_startup_config(data_dir.path()).is_err());
}

#[test]
fn conversation_history_replays_context_checkpoints_as_system_messages() {
    let checkpoint =
        ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::Notice)
            .with_text("[context checkpoint]\nGoal: preserve state")
            .with_marker(ark_contract::ItemMarker::ContextCheckpoint)
            .completed();

    assert_eq!(
        conversation_history(&[checkpoint]),
        vec![ModelMessage::system(
            "[context checkpoint]\nGoal: preserve state"
        )]
    );
}

#[test]
fn conversation_history_starts_at_the_latest_checkpoint() {
    let checkpoint = |text: &str| {
        ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::Notice)
            .with_text(text)
            .with_marker(ark_contract::ItemMarker::ContextCheckpoint)
            .completed()
    };
    let user = |text: &str| {
        ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::UserMessage)
            .with_text(text)
            .completed()
    };
    let items = vec![
        user("ancient request"),
        checkpoint("[context checkpoint] first"),
        user("middle request"),
        checkpoint("[context checkpoint] latest"),
        user("newest request"),
    ];

    // Only the newest boundary and rows written after it reach the model; the
    // original rows stay on disk for rewind/export.
    assert_eq!(
        conversation_history(&items),
        vec![
            ModelMessage::system("[context checkpoint] latest"),
            ModelMessage::user("newest request"),
        ]
    );
}

#[test]
fn conversation_history_without_a_checkpoint_replays_everything() {
    let items = vec![
        ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::UserMessage)
            .with_text("first")
            .completed(),
        ark_contract::ThreadItem::new(
            TurnId::new(),
            ark_contract::item::ItemKind::AssistantMessage,
        )
        .with_text("second")
        .completed(),
    ];
    assert_eq!(
        conversation_history(&items),
        vec![
            ModelMessage::user("first"),
            ModelMessage::assistant("second")
        ]
    );
}

#[test]
fn plan_seed_restates_goal_and_todo_status() {
    use ark_contract::{PlanView, TodoEntry, TodoStatus};
    let plan = PlanView {
        goal: Some("repair the parser".into()),
        todos: vec![
            TodoEntry {
                id: "1".into(),
                title: "read files".into(),
                status: TodoStatus::Completed,
            },
            TodoEntry {
                id: "2".into(),
                title: "fix wire format".into(),
                status: TodoStatus::InProgress,
            },
            TodoEntry {
                id: "3".into(),
                title: "run tests".into(),
                status: TodoStatus::Pending,
            },
        ],
    };
    let message = super::history::plan_seed_message(&plan).expect("a plan produces a seed");
    assert!(message.content.contains("Goal: repair the parser"));
    assert!(message.content.contains("- [x] read files"));
    assert!(message.content.contains("- [~] fix wire format"));
    assert!(message.content.contains("- [ ] run tests"));
    assert_eq!(
        super::history::plan_seed_message(&PlanView::default()),
        None
    );
}

#[test]
fn project_instructions_ride_both_seed_paths_dynamic_tail() {
    let dir = std::env::temp_dir().join(format!("ark-wiring-instructions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("fixture dir");
    std::fs::write(dir.join("AGENTS.md"), "always run cargo clippy first").expect("writable");

    // Shared helper: the same call site shape both paths use.
    let mut parent_like = super::turn_common::seed_prefix(&dir);
    super::turn_common::push_project_instructions(&dir, &mut parent_like);
    assert!(parent_like
        .iter()
        .any(|message| message.content.contains("always run cargo clippy first")));

    // The instruction message is a dynamic-tail system message, never part of
    // the static prefix: a workspace-specific text inside the prefix would
    // break the byte-stable prompt-cache invariant.
    let bare_prefix = super::turn_common::seed_prefix(&dir);
    assert_eq!(bare_prefix.len(), 2);
    assert!(bare_prefix
        .iter()
        .all(|message| !message.content.contains("cargo clippy")));

    // A workspace without instruction files adds no message at all.
    let empty =
        std::env::temp_dir().join(format!("ark-wiring-no-instructions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).expect("fixture dir");
    let mut no_instructions = Vec::new();
    super::turn_common::push_project_instructions(&empty, &mut no_instructions);
    assert!(no_instructions.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&empty);
}

/// A folded tool item as the store projects it: one `ToolCall` entry whose
/// payload embeds the result (see `apply_tool_finished`).
fn tool_item(
    call_id: &str,
    tool: &str,
    output: Option<&str>,
    status: ark_contract::item::ItemStatus,
) -> ark_contract::ThreadItem {
    let mut item =
        ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::ToolCall);
    item.status = status;
    item.tool = Some(ark_contract::item::ToolPayload {
        call_id: call_id.into(),
        tool: tool.into(),
        arguments: Some(serde_json::json!({"path": "a.txt"})),
        output: output.map(Into::into),
        ok: Some(status == ark_contract::item::ItemStatus::Completed),
        duration_ms: None,
    });
    item
}

#[test]
fn paired_tool_rows_fold_into_the_assistant_history() {
    let items = vec![
        ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::UserMessage)
            .with_text("hi")
            .completed(),
        ark_contract::ThreadItem::new(
            TurnId::new(),
            ark_contract::item::ItemKind::AssistantMessage,
        )
        .with_text("calling")
        .completed(),
        tool_item(
            "c1",
            "read",
            Some("file body"),
            ark_contract::item::ItemStatus::Completed,
        ),
    ];
    assert_eq!(
        conversation_history(&items),
        vec![
            ModelMessage::user("hi"),
            ModelMessage::assistant_with_tool_calls(
                "calling",
                vec![ark_ports::ToolCallRef {
                    call_id: "c1".into(),
                    name: "read".into(),
                    arguments: serde_json::json!({"path": "a.txt"}),
                }],
            ),
            ModelMessage {
                role: ModelRole::Tool,
                images: Vec::new(),
                content: "file body".into(),
                tool_call_id: Some("c1".into()),
                name: Some("read".into()),
                tool_calls: Vec::new(),
            },
        ]
    );
}

#[test]
fn non_terminal_tool_items_are_not_replayed() {
    let streaming = tool_item(
        "c1",
        "read",
        None,
        ark_contract::item::ItemStatus::Streaming,
    );
    assert_eq!(conversation_history(&[streaming]), Vec::new());
}

#[test]
fn failed_tool_results_are_replayed_with_empty_output_fallback() {
    let failed = tool_item("c1", "bash", None, ark_contract::item::ItemStatus::Failed);
    let history = conversation_history(&[failed]);
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content, "");
    assert_eq!(history[1].tool_call_id.as_deref(), Some("c1"));
}

#[test]
fn oversized_tool_output_is_truncated_at_16kib_bytes() {
    let big = "x".repeat(20 * 1024);
    let items = vec![tool_item(
        "c1",
        "read",
        Some(&big),
        ark_contract::item::ItemStatus::Completed,
    )];
    let history = conversation_history(&items);
    assert_eq!(history[1].content.len(), 16 * 1024);
}

#[test]
fn tool_history_keeps_the_time_order_of_items() {
    let items = vec![
        ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::UserMessage)
            .with_text("q1")
            .completed(),
        tool_item(
            "c1",
            "read",
            Some("r1"),
            ark_contract::item::ItemStatus::Completed,
        ),
        ark_contract::ThreadItem::new(
            TurnId::new(),
            ark_contract::item::ItemKind::AssistantMessage,
        )
        .with_text("answer")
        .completed(),
        ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::UserMessage)
            .with_text("q2")
            .completed(),
    ];
    let history = conversation_history(&items);
    let roles: Vec<ModelRole> = history.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![
            ModelRole::User,
            ModelRole::Assistant,
            ModelRole::Tool,
            ModelRole::Assistant,
            ModelRole::User
        ]
    );
    // The tool call has no leading assistant text, so it lands on a
    // synthetic empty-content assistant row before its result.
    assert_eq!(history[1].content, "");
    assert_eq!(history[2].content, "r1");
    assert_eq!(history[3].content, "answer");
}

use ark_common::ChildId;
use ark_contract::RuntimeEvent;
use ark_llm::ProviderConfig;
use chrono::Utc;

/// A fresh random data directory, removed when the guard drops.
///
/// `tempfile` names it uniquely, so the completion-inbox tests running in
/// parallel can never pick the same store directory or data lock.
struct TestDataDir(tempfile::TempDir);

impl TestDataDir {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("ark-completion-inbox-")
            .tempdir()
            .expect("temp dir is creatable");
        Self(dir)
    }

    fn path(&self) -> &std::path::Path {
        self.0.path()
    }
}

#[tokio::test]
async fn child_completion_events_are_loaded_as_the_next_turn_system_source() {
    let data_dir = TestDataDir::new();
    let store = FileSessionStore::open(StoreConfig::new(data_dir.path())).unwrap();
    let thread = store.create_thread("parent").await.unwrap().thread_id;
    store
        .append_event(
            thread,
            RuntimeEvent::ChildCompleted {
                seq: 0,
                thread_id: thread,
                timestamp: Utc::now(),
                notification_id: "child:one".to_string(),
                child_id: ChildId::new(),
                label: "inspect runtime".to_string(),
                status: ark_common::ChildRunStatus::Completed,
                summary: Some("The runtime is healthy.".to_string()),
                error: None,
                usage: None,
                tool_invocations: None,
            },
        )
        .await
        .unwrap();

    let inbox = completion_inbox(&store, thread).await.unwrap();
    assert_eq!(inbox.notification_ids(), &["child:one".to_string()]);
    assert_eq!(
        inbox.system_message().unwrap().role,
        ark_ports::ModelRole::System
    );
}

#[tokio::test]
async fn completed_parent_turn_acknowledges_the_injected_completion_and_skips_it_next_time() {
    let data_dir = TestDataDir::new();
    let store = FileSessionStore::open(StoreConfig::new(data_dir.path())).unwrap();
    let thread = store.create_thread("parent").await.unwrap().thread_id;
    let turn = TurnId::new();

    store
        .append_event(
            thread,
            RuntimeEvent::ChildCompleted {
                seq: 0,
                thread_id: thread,
                timestamp: Utc::now(),
                notification_id: "child:one".to_string(),
                child_id: ChildId::new(),
                label: "inspect runtime".to_string(),
                status: ark_common::ChildRunStatus::Completed,
                summary: Some("The runtime is healthy.".to_string()),
                error: None,
                usage: None,
                tool_invocations: None,
            },
        )
        .await
        .unwrap();
    let inbox = completion_inbox(&store, thread).await.unwrap();
    assert!(
        inbox.system_message().is_some(),
        "the turn receives a system source"
    );

    let end =
        finish_parent_turn(TurnOutcome::Completed, &store, &store, thread, turn, &inbox).await;

    assert!(matches!(end, TurnEnd::Completed));
    assert!(completion_inbox(&store, thread)
        .await
        .unwrap()
        .notification_ids()
        .is_empty());
}

#[tokio::test]
async fn completed_parent_turn_reports_another_bounded_inbox_page() {
    let data_dir = TestDataDir::new();
    let store = FileSessionStore::open(StoreConfig::new(data_dir.path())).unwrap();
    let thread = store.create_thread("parent").await.unwrap().thread_id;
    for index in 0..9 {
        store
            .append_event(
                thread,
                RuntimeEvent::ChildCompleted {
                    seq: 0,
                    thread_id: thread,
                    timestamp: Utc::now(),
                    notification_id: format!("child:{index}"),
                    child_id: ChildId::new(),
                    label: format!("worker {index}"),
                    status: ark_common::ChildRunStatus::Completed,
                    summary: Some("done".to_string()),
                    error: None,
                    usage: None,
                    tool_invocations: None,
                },
            )
            .await
            .unwrap();
    }

    let first = completion_inbox(&store, thread).await.unwrap();
    assert_eq!(first.notification_ids().len(), 8);
    let first_end = finish_parent_turn(
        TurnOutcome::Completed,
        &store,
        &store,
        thread,
        TurnId::new(),
        &first,
    )
    .await;
    assert!(matches!(
        first_end,
        TurnEnd::CompletedWithPendingCompletions
    ));

    let second = completion_inbox(&store, thread).await.unwrap();
    assert_eq!(second.notification_ids().len(), 1);
    let second_end = finish_parent_turn(
        TurnOutcome::Completed,
        &store,
        &store,
        thread,
        TurnId::new(),
        &second,
    )
    .await;
    assert!(matches!(second_end, TurnEnd::Completed));
    assert!(completion_inbox(&store, thread)
        .await
        .unwrap()
        .notification_ids()
        .is_empty());
}

#[tokio::test]
async fn failed_parent_turn_leaves_the_completion_for_redelivery() {
    let data_dir = TestDataDir::new();
    let store = FileSessionStore::open(StoreConfig::new(data_dir.path())).unwrap();
    let thread = store.create_thread("parent").await.unwrap().thread_id;
    store
        .append_event(
            thread,
            RuntimeEvent::ChildCompleted {
                seq: 0,
                thread_id: thread,
                timestamp: Utc::now(),
                notification_id: "child:one".to_string(),
                child_id: ChildId::new(),
                label: "inspect runtime".to_string(),
                status: ark_common::ChildRunStatus::Failed,
                summary: None,
                error: Some("child failed".to_string()),
                usage: None,
                tool_invocations: None,
            },
        )
        .await
        .unwrap();
    let inbox = completion_inbox(&store, thread).await.unwrap();

    let end = finish_parent_turn(
        TurnOutcome::Failed {
            message: "model failed".to_string(),
            model_request_failed: false,
        },
        &store,
        &store,
        thread,
        TurnId::new(),
        &inbox,
    )
    .await;

    assert!(matches!(end, TurnEnd::Failed { message, .. } if message == "model failed"));
    assert_eq!(
        completion_inbox(&store, thread)
            .await
            .unwrap()
            .notification_ids(),
        &["child:one".to_string()]
    );
}

#[test]
fn turn_limits_derive_the_threshold_from_the_provider_window() {
    let config = RuntimeConfig::default();
    let mut provider = ProviderConfig::new("p", "m");
    provider.context_window_tokens = Some(32_000);
    provider.max_tokens = Some(4_096);

    let limits = super::turn_common::limits_for_turn(&config, Some(&provider));

    // 80% of (32_000 - 4_096): tuning no longer carries a threshold field.
    assert_eq!(limits.context_limit_tokens, 22_323);
}

#[test]
fn undeclared_provider_window_falls_back_to_the_256k_default() {
    let config = RuntimeConfig::default();
    let limits = super::turn_common::limits_for_turn(&config, None);
    // 80% of the 256K default window: compaction stays on for undeclared
    // windows, and the meter shown in the session divides by the same value.
    assert_eq!(
        limits.context_limit_tokens,
        ark_llm::DEFAULT_CONTEXT_WINDOW_TOKENS * 4 / 5
    );
}

#[test]
fn zero_turn_budgets_map_to_unlimited_limits() {
    let config = RuntimeConfig {
        tuning: ark_config::TuningConfig {
            max_rounds: 0,
            max_tokens: 0,
            ..ark_config::TuningConfig::default()
        },
        ..RuntimeConfig::default()
    };
    let limits = super::turn_common::limits_for_turn(&config, None);
    assert_eq!(limits.max_rounds, u32::MAX);
    assert_eq!(limits.max_tokens, u64::MAX);
}

#[test]
fn positive_turn_budgets_are_passed_through_unchanged() {
    let config = RuntimeConfig {
        tuning: ark_config::TuningConfig {
            max_rounds: 500,
            max_tokens: 5_000_000,
            ..ark_config::TuningConfig::default()
        },
        ..RuntimeConfig::default()
    };
    let limits = super::turn_common::limits_for_turn(&config, None);
    assert_eq!(limits.max_rounds, 500);
    assert_eq!(limits.max_tokens, 5_000_000);
}

#[tokio::test]
async fn startup_aborts_a_parent_turn_left_open_by_a_previous_process() {
    let data_dir = TestDataDir::new();
    let store = FileSessionStore::open(StoreConfig::new(data_dir.path())).unwrap();
    let thread = store.create_thread("parent").await.unwrap().thread_id;
    let turn = TurnId::new();
    store
        .append_event(
            thread,
            RuntimeEvent::TurnStarted {
                seq: 0,
                thread_id: thread,
                turn_id: turn,
                timestamp: Utc::now(),
                auto_continuation: false,
                attachment_ids: Vec::new(),
                attachment_outcomes: Vec::new(),
            },
        )
        .await
        .unwrap();

    assert_eq!(abort_interrupted_turns(&store).await.unwrap(), 1);
    assert_eq!(abort_interrupted_turns(&store).await.unwrap(), 0);

    let events = store.replay_interaction_events(thread).await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::TurnAborted { turn_id, reason, .. }
            if *turn_id == turn && reason == "runtime restarted before turn could finish"
    )));
}

#[test]
fn conversation_history_keeps_visible_messages_and_excludes_reasoning() {
    let turn = TurnId::new();
    let items = vec![
        ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::UserMessage)
            .with_text("first question"),
        ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::Reasoning)
            .with_text("private reasoning"),
        ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::AssistantMessage)
            .with_text("first answer"),
    ];

    let history = conversation_history(&items);

    assert_eq!(history.len(), 2);
    assert_eq!(history[0], ModelMessage::user("first question"));
    assert_eq!(history[1], ModelMessage::assistant("first answer"));
}

#[test]
fn user_message_carries_previews_in_content_and_images_in_parts() {
    let seed = crate::attachment_seed::AttachmentSeed {
        images: vec![ark_ports::ModelImage {
            mime_type: "image/png".into(),
            data: "aGk=".into(),
        }],
        preview: "[attachment: notes.txt]\nbody".into(),
        outcomes: Vec::new(),
    };
    let message = user_message_with_attachments("look at this".into(), &seed);
    assert_eq!(
        message.content,
        "look at this\n[attachment: notes.txt]\nbody"
    );
    assert_eq!(message.images.len(), 1);
    assert_eq!(message.images[0].mime_type, "image/png");

    // A turn without attachments keeps the text-only shape.
    let plain = user_message_with_attachments("hello".into(), &Default::default());
    assert!(plain.images.is_empty());
    assert_eq!(plain.content, "hello");
}

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
    let wiring = crate::wiring::build(&args).await.expect("runtime build");
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
