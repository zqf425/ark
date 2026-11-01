//! Wiring tests: startup config, turn lifecycle, history replay and tool sets.

use super::history::conversation_history;
use super::tools::{build_child_tools, build_tools};
use super::turns::{
    abort_interrupted_turns, completion_inbox, finish_parent_turn, AgentTurnRunner,
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
            .completed();

    assert_eq!(
        conversation_history(&[checkpoint]),
        vec![ModelMessage::system(
            "[context checkpoint]\nGoal: preserve state"
        )]
    );
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
                status: "completed".to_string(),
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
                status: "completed".to_string(),
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
                    status: "completed".to_string(),
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
                status: "failed".to_string(),
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
fn provider_context_window_caps_the_next_turn_and_reserves_requested_output() {
    let config = RuntimeConfig {
        tuning: ark_config::TuningConfig {
            context_limit_tokens: 100_000,
            ..ark_config::TuningConfig::default()
        },
        ..RuntimeConfig::default()
    };
    let mut provider = ProviderConfig::new("p", "m");
    provider.context_window_tokens = Some(32_000);
    provider.max_tokens = Some(4_096);

    let limits = AgentTurnRunner::limits_for_turn(&config, Some(&provider));

    assert_eq!(limits.context_limit_tokens, 27_904);
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
fn request_user_input_is_registered_for_both_agent_tool_sets() {
    let workspace = tempfile::tempdir().expect("temp dir is creatable");
    let skills = Arc::new(ark_skills::SkillRuntime::new(
        ark_skills::SkillsConfig::default(),
        workspace.path().to_path_buf(),
    ));
    let child_tools = build_child_tools(Arc::clone(&skills)).expect("child tools build");
    assert!(child_tools
        .names()
        .contains(&"request_user_input".to_string()));

    // The parent set adds `delegate_task` on top of the child set.
    let parent_tools =
        build_tools(Arc::new(TestSubagentRunner), Arc::clone(&skills)).expect("tools build");
    let names = parent_tools.names();
    assert!(names.contains(&"request_user_input".to_string()));
    assert!(names.contains(&"bash".to_string()));
    assert!(names.contains(&"delegate_task".to_string()));

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
