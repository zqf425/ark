use super::ContextCheckpoint;
use ark_ports::{ModelMessage, ToolCallRef};

#[test]
fn tool_evidence_keeps_typed_arguments_and_bounded_results() {
    let checkpoint = ContextCheckpoint::from_messages(&[
        ModelMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRef {
                call_id: "call-1".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({
                    "command": "cargo test -p ark-agent",
                    "cwd": "/repo",
                    "content": "bulk payload that must not be copied",
                }),
            }],
        ),
        ModelMessage {
            role: ark_ports::ModelRole::Tool,
            images: Vec::new(),
            content: format!("{}\nFINAL-DIGEST-42", "output ".repeat(500)),
            tool_call_id: Some("call-1".to_string()),
            name: Some("bash".to_string()),
            tool_calls: Vec::new(),
        },
    ]);

    let rendered = checkpoint.render();
    assert!(rendered.contains("[tool evidence]"));
    assert!(rendered.contains("bash(command=cargo test -p ark-agent, cwd=/repo)"));
    assert!(!rendered.contains("bulk payload"));
    assert!(rendered.contains("truncated"));
    assert!(rendered.contains("FINAL-DIGEST-42"));
}

#[test]
fn overlong_single_line_user_request_keeps_both_ends() {
    let request = format!("BEGIN-{} END-{}", "a".repeat(2_000), "Z".repeat(2_000));
    let rendered = ContextCheckpoint::from_messages(&[ModelMessage::user(request)]).render();

    assert!(rendered.contains("BEGIN-"));
    assert!(rendered.contains("ZZZZ"));
    assert!(rendered.contains("[... truncated ...]"));
    assert!(rendered.lines().any(|line| line.chars().count() <= 242));
}

#[test]
fn overlong_failure_lines_keep_both_ends_and_still_collapse_on_absorption() {
    let failure = format!(
        "Error: BEGIN-{} END-{}",
        "a".repeat(2_000),
        "Z".repeat(2_000)
    );
    let first = ContextCheckpoint::from_texts([failure.as_str(), failure.as_str()]).render();
    assert!(first.contains("BEGIN-"), "{first}");
    assert!(first.contains("ZZZZ"), "{first}");
    assert!(first.contains("[... truncated ...]"), "{first}");
    assert!(first.contains("(repeated x2)"), "{first}");

    let second = ContextCheckpoint::from_messages(&[ModelMessage::system(first)]).render();
    assert!(second.contains("BEGIN-"), "{second}");
    assert!(second.contains("ZZZZ"), "{second}");
    assert_eq!(second.matches("Error: BEGIN-").count(), 1, "{second}");
    assert_eq!(second.matches("(repeated x2)").count(), 1, "{second}");
}

#[test]
fn tool_evidence_survives_checkpoint_absorption() {
    let first = ContextCheckpoint::from_messages(&[ModelMessage::assistant_with_tool_calls(
        "",
        vec![ToolCallRef {
            call_id: "call-1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({ "path": "src/main.rs" }),
        }],
    )])
    .render();
    let second = ContextCheckpoint::from_messages(&[ModelMessage::system(first)]).render();

    assert!(second.contains("read(path=src/main.rs)"));
}
