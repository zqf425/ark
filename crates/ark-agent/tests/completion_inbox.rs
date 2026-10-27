use ark_agent::CompletionInbox;
use ark_common::{ChildId, ThreadId, TurnId};
use ark_contract::RuntimeEvent;
use chrono::Utc;

fn completed(thread_id: ThreadId, notification_id: &str) -> RuntimeEvent {
    RuntimeEvent::ChildCompleted {
        seq: 1,
        thread_id,
        timestamp: Utc::now(),
        notification_id: notification_id.to_string(),
        child_id: ChildId::new(),
        label: "inspect runtime".to_string(),
        status: "completed".to_string(),
        summary: Some("The runtime is healthy.".to_string()),
        error: None,
        usage: None,
        tool_invocations: None,
    }
}

#[test]
fn completion_events_become_one_deduplicated_system_source() {
    let thread_id = ThreadId::new();
    let first = completed(thread_id, "child:first");
    let duplicate = first.clone();
    let inbox = CompletionInbox::from_events(&[first, duplicate]);

    assert_eq!(inbox.notification_ids(), &["child:first".to_string()]);
    let source = inbox.system_message().expect("a completion source");
    assert_eq!(source.role, ark_ports::ModelRole::System);
    assert!(source.content.contains("[child agent results]"));
    assert!(source.content.contains("child:first"));
    assert!(source.content.contains("The runtime is healthy."));
}

#[test]
fn acknowledged_completion_is_not_replayed_to_a_later_turn() {
    let thread_id = ThreadId::new();
    let completed = completed(thread_id, "child:acknowledged");
    let acknowledged = RuntimeEvent::ChildCompletionAcknowledged {
        seq: 2,
        thread_id,
        turn_id: TurnId::new(),
        timestamp: Utc::now(),
        notification_id: "child:acknowledged".to_string(),
    };

    let inbox = CompletionInbox::from_events(&[completed, acknowledged]);
    assert!(inbox.notification_ids().is_empty());
    assert!(inbox.system_message().is_none());
}

#[test]
fn inbox_leaves_excess_results_for_later_turns() {
    let thread_id = ThreadId::new();
    let events: Vec<_> = (0..9)
        .map(|index| completed(thread_id, &format!("child:{index}")))
        .collect();

    let inbox = CompletionInbox::from_events(&events);
    assert_eq!(inbox.notification_ids().len(), 8);
    assert!(!inbox.system_message().unwrap().content.contains("child:8"));
}

#[test]
fn inbox_bounds_each_child_detail_before_building_model_context() {
    let thread_id = ThreadId::new();
    let mut event = completed(thread_id, "child:long");
    if let RuntimeEvent::ChildCompleted { summary, .. } = &mut event {
        *summary = Some("x".repeat(1_201));
    }

    let source = CompletionInbox::from_events(&[event])
        .system_message()
        .unwrap();
    assert!(source.content.contains(&"x".repeat(1_200)));
    assert!(!source.content.contains(&"x".repeat(1_201)));
}
