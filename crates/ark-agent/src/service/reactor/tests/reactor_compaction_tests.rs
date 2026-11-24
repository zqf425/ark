use super::*;

#[test]
fn dynamic_system_context_survives_repeated_compaction() {
    let mut reactor = reactor(Arc::new(MemoryWriter::new()));
    reactor.history = vec![
        ModelMessage::system("static prefix"),
        ModelMessage::user("Goal: preserve the original task"),
        ModelMessage::assistant("old answer"),
        ModelMessage::system("project instructions: always run cargo fmt"),
        ModelMessage::system("user memory: deploy target is staging"),
        ModelMessage::user("recent request"),
        ModelMessage::assistant("recent answer"),
    ];

    reactor.compact_history(2);
    reactor.history.push(ModelMessage::user("new request"));
    reactor.history.push(ModelMessage::assistant("new answer"));
    reactor.compact_history(2);

    for context in [
        "static prefix",
        "project instructions: always run cargo fmt",
        "user memory: deploy target is staging",
    ] {
        assert_eq!(
            reactor
                .history
                .iter()
                .filter(|message| message.content == context)
                .count(),
            1,
            "protected context must survive every rebuild: {context}"
        );
    }
    assert!(reactor.history.iter().any(|message| {
        message.content.starts_with("[context checkpoint")
            && message.content.contains("preserve the original task")
    }));
}

#[test]
fn compaction_budget_includes_tools_dynamic_context_and_staged_steering() {
    let mut reactor = reactor(Arc::new(MemoryWriter::new()));
    reactor.machine = TurnMachine::new(crate::domain::TurnLimits {
        context_limit_tokens: 3_000,
        ..crate::domain::TurnLimits::tiny()
    });
    reactor.tools_schema = vec![serde_json::json!({
        "name": "read",
        "description": "schema ".repeat(200),
    })];
    let queue = Arc::new(SteerQueue::new());
    queue.push("steering ".repeat(160));
    reactor = reactor.with_steering(queue);
    reactor.drain_steering();
    let oversized = "oversized-tail-marker ".repeat(400);
    reactor.history = vec![
        ModelMessage::system("static prefix"),
        ModelMessage::user("old request"),
        ModelMessage::assistant("old answer"),
        ModelMessage::system("dynamic-context ".repeat(160)),
        ModelMessage::user(&oversized),
        ModelMessage::assistant("latest answer"),
    ];

    let (removed, _) = reactor.compact_history(2);

    assert!(removed > 0);
    assert!(!reactor
        .history
        .iter()
        .any(|message| message.content == oversized));
    assert!(reactor
        .history
        .iter()
        .any(|message| message.content == "latest answer"));
    assert!(TokenEstimator::new().estimate_request(&reactor.request()) < 3_000);
}

#[test]
fn checkpoint_chain_counts_only_new_failure_occurrences() {
    let mut reactor = reactor(Arc::new(MemoryWriter::new()));
    reactor.history = vec![
        ModelMessage::system("static prefix"),
        ModelMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRef {
                call_id: "c1".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }],
        ),
        tool_message("c1", "bash", "Error: exit status 101"),
        ModelMessage::user("keep this request"),
    ];
    reactor.compact_history(1);
    reactor
        .history
        .push(ModelMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRef {
                call_id: "c2".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }],
        ));
    reactor
        .history
        .push(tool_message("c2", "bash", "Error: exit status 101"));
    reactor.history.push(ModelMessage::user("newest request"));

    reactor.compact_history(1);

    let checkpoint = reactor
        .history
        .iter()
        .find(|message| message.content.starts_with("[context checkpoint"))
        .expect("checkpoint");
    assert!(checkpoint.content.contains("(repeated x2)"));
    assert!(!checkpoint.content.contains("(repeated x3)"));
    assert!(checkpoint.content.contains("bash(command=cargo test)"));
}

#[test]
fn retained_failure_tail_is_not_counted_again_at_the_next_boundary() {
    let mut reactor = reactor(Arc::new(MemoryWriter::new()));
    reactor.history = vec![
        ModelMessage::system("static prefix"),
        ModelMessage::user("request before the failed call"),
        ModelMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRef {
                call_id: "c1".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }],
        ),
        tool_message("c1", "bash", "Error: exit status 101"),
    ];

    reactor.compact_history(2);
    let first = reactor
        .history
        .iter()
        .find(|message| message.content.starts_with("[context checkpoint"))
        .expect("first checkpoint");
    assert!(first
        .content
        .contains("[retained tail already summarized: 2; fingerprint: "));
    reactor.history.push(ModelMessage::user("new request"));

    reactor.compact_history(1);

    let checkpoint = reactor
        .history
        .iter()
        .find(|message| message.content.starts_with("[context checkpoint"))
        .expect("second checkpoint");
    assert!(checkpoint.content.contains("Error: exit status 101"));
    assert!(!checkpoint.content.contains("(repeated x2)"));
    assert!(checkpoint.content.contains("new request"));
}

#[test]
fn replayed_suffix_is_not_skipped_by_the_previous_tail_fingerprint() {
    let mut reactor = reactor(Arc::new(MemoryWriter::new()));
    reactor.history = vec![
        ModelMessage::system("static prefix"),
        ModelMessage::user("request before the failed call"),
        ModelMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRef {
                call_id: "c1".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({ "command": "cargo test" }),
            }],
        ),
        tool_message("c1", "bash", "Error: exit status 101"),
    ];
    reactor.compact_history(2);
    let checkpoint = reactor
        .history
        .iter()
        .find(|message| message.content.starts_with("[context checkpoint"))
        .expect("checkpoint")
        .clone();

    reactor.history = vec![
        checkpoint,
        ModelMessage::user("first request after replay"),
        ModelMessage::assistant("answer after replay"),
    ];
    reactor.compact_history(1);

    let checkpoint = reactor
        .history
        .iter()
        .find(|message| message.content.starts_with("[context checkpoint"))
        .expect("replayed checkpoint");
    assert!(checkpoint.content.contains("first request after replay"));
    assert!(checkpoint.content.contains("Error: exit status 101"));
    assert!(!checkpoint.content.contains("(repeated x2)"));
}
