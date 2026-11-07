use super::*;
use crate::domain::diagnostics::MemoryDiagnostics;
use crate::store::tests::{event_at, text_delta, TempDir};
use ark_common::{ChildId, ItemId, TurnId};
use chrono::Utc;

fn state(dir: &TempDir, thread: ThreadId) -> WriterState {
    WriterState::open(
        thread,
        ThreadPaths::new(dir.path().join("thread")),
        StoreLimits::for_tests(),
        Arc::new(MemoryDiagnostics::new()),
    )
    .unwrap()
}

fn child_completed(thread: ThreadId, notification_id: &str) -> RuntimeEvent {
    RuntimeEvent::ChildCompleted {
        seq: 0,
        thread_id: thread,
        timestamp: Utc::now(),
        notification_id: notification_id.to_string(),
        child_id: ChildId::new(),
        label: "inspect runtime".to_string(),
        status: ark_common::ChildRunStatus::Completed,
        summary: Some("healthy".to_string()),
        error: None,
        usage: None,
        tool_invocations: None,
    }
}

#[test]
fn open_refuses_a_log_with_mid_file_garbage_without_modifying_it() {
    let dir = TempDir::new("writer-open-corrupt");
    let thread = ThreadId::new();
    let paths = ThreadPaths::new(dir.path().join("thread"));
    crate::store::fs_util::ensure_dir(paths.dir()).unwrap();
    let mut text = String::new();
    text.push_str(&encode_event(&event_at(thread, 1)).unwrap());
    text.push_str("garbage\n");
    text.push_str(&encode_event(&event_at(thread, 2)).unwrap());
    std::fs::write(paths.events_jsonl(), &text).unwrap();

    let err = match WriterState::open(
        thread,
        paths.clone(),
        StoreLimits::for_tests(),
        Arc::new(MemoryDiagnostics::new()),
    ) {
        Ok(_) => panic!("the writer must not start on a corrupt log"),
        Err(err) => err,
    };
    assert_eq!(err.code, ark_common::ArkErrorCode::StoreFailure);
    assert!(err.message.contains("1 malformed"), "{}", err.message);
    assert!(
        err.message
            .contains(&paths.events_jsonl().display().to_string()),
        "the error must name the file: {}",
        err.message
    );
    // No truncation and no append: the on-disk log is byte-identical.
    assert_eq!(
        std::fs::read(paths.events_jsonl()).unwrap(),
        text.as_bytes()
    );
}

#[test]
fn compaction_keeps_an_unacknowledged_child_completion() {
    let dir = TempDir::new("completion-compaction");
    let thread = ThreadId::new();
    let mut state = state(&dir, thread);
    state.append(event_at(thread, 1)).unwrap();
    state
        .append(child_completed(thread, "child:pending"))
        .unwrap();
    for _ in 0..3 {
        state.append(event_at(thread, 1)).unwrap();
    }

    let compacted_to = state.compact(2).unwrap();

    assert_eq!(compacted_to, Some(1));
    assert!(state.events().iter().any(|event| {
        matches!(
            event,
            RuntimeEvent::ChildCompleted { notification_id, .. }
                if notification_id == "child:pending"
        )
    }));
}

#[test]
fn completion_query_excludes_unrelated_event_payloads() {
    let dir = TempDir::new("completion-query");
    let thread = ThreadId::new();
    let mut state = state(&dir, thread);
    state.append(event_at(thread, 1)).unwrap();
    state.append(child_completed(thread, "child:one")).unwrap();

    let events = state.child_completion_events();

    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], RuntimeEvent::ChildCompleted { .. }));
}

#[test]
fn interaction_query_excludes_transcript_payloads() {
    let dir = TempDir::new("interaction-query");
    let thread = ThreadId::new();
    let turn = ark_common::TurnId::new();
    let mut state = state(&dir, thread);
    state.append(event_at(thread, 1)).unwrap();
    state
        .append(RuntimeEvent::TurnStarted {
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: Utc::now(),
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        })
        .unwrap();
    state
        .append(RuntimeEvent::ApprovalRequested {
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: Utc::now(),
            approval_id: "approval-1".into(),
            tool: "bash".into(),
            summary: "run command".into(),
        })
        .unwrap();

    let events = state.interaction_events();

    assert_eq!(events.len(), 2);
    assert!(events
        .iter()
        .any(|event| matches!(event, RuntimeEvent::TurnStarted { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, RuntimeEvent::ApprovalRequested { .. })));
    assert!(events
        .iter()
        .all(|event| !matches!(event, RuntimeEvent::Error { .. })));
}

#[test]
fn rewind_physically_discards_the_suffix_and_notifies_subscribers() {
    let dir = TempDir::new("rewind-physical-truncation");
    let thread = ThreadId::new();
    let mut state = state(&dir, thread);
    for _ in 0..4 {
        state.append(event_at(thread, 1)).unwrap();
    }
    let mut subscription = state.snapshot_and_subscribe(0).unwrap();

    let result = state.rewind(3, None).unwrap();

    assert_eq!(result.last_seq, 5);
    assert!(state
        .events()
        .iter()
        .all(|event| event.seq() < 3 || event.seq() == 5));
    assert!(matches!(
        subscription.live.try_recv(),
        Ok(RuntimeEvent::ThreadRewound { .. })
    ));
}

#[test]
fn rewind_marker_survives_compaction_of_its_notification() {
    let dir = TempDir::new("rewind-marker-compaction");
    let thread = ThreadId::new();
    let mut state = state(&dir, thread);
    for _ in 0..4 {
        state.append(event_at(thread, 1)).unwrap();
    }

    state.rewind(3, None).unwrap();
    state.append(event_at(thread, 1)).unwrap();
    state.append(event_at(thread, 1)).unwrap();
    state.compact(1).unwrap();

    assert!(state.rewind_record(Some(3), None).is_some());
}

#[test]
fn rewind_rejects_any_existing_compaction_sidecar() {
    let dir = TempDir::new("rewind-compaction-sidecar");
    let thread = ThreadId::new();
    let mut state = state(&dir, thread);
    state.append(event_at(thread, 1)).unwrap();
    std::fs::write(state.paths().compacted_jsonl(), b"").unwrap();

    let error = state.rewind(1, None).unwrap_err();

    assert_eq!(error.code, ark_common::ArkErrorCode::Conflict);
    assert!(error.message.contains("compacted"));
}

#[test]
fn rewind_rejects_a_thread_with_a_running_persisted_turn() {
    let dir = TempDir::new("rewind-running-turn");
    let thread = ThreadId::new();
    let turn = ark_common::TurnId::new();
    let mut state = state(&dir, thread);
    state
        .append(RuntimeEvent::TurnStarted {
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: Utc::now(),
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        })
        .unwrap();

    assert_eq!(
        state.rewind(1, Some(turn)).unwrap_err().code,
        ark_common::ArkErrorCode::Conflict
    );
}

#[test]
fn acknowledged_child_completion_no_longer_blocks_compaction() {
    let dir = TempDir::new("acknowledged-completion-compaction");
    let thread = ThreadId::new();
    let mut state = state(&dir, thread);
    state.append(event_at(thread, 1)).unwrap();
    state
        .append(child_completed(thread, "child:acknowledged"))
        .unwrap();
    state
        .append(RuntimeEvent::ChildCompletionAcknowledged {
            seq: 0,
            thread_id: thread,
            turn_id: ark_common::TurnId::new(),
            timestamp: Utc::now(),
            notification_id: "child:acknowledged".to_string(),
        })
        .unwrap();
    for _ in 0..3 {
        state.append(event_at(thread, 1)).unwrap();
    }

    let compacted_to = state.compact(2).unwrap();

    assert_eq!(compacted_to, Some(4));
    assert!(state
        .events()
        .iter()
        .all(|event| !event.is_child_completion_lifecycle()));
}

#[test]
fn compaction_keeps_an_acknowledgement_for_a_retained_duplicate_completion() {
    let dir = TempDir::new("duplicate-completion-compaction");
    let thread = ThreadId::new();
    let mut state = state(&dir, thread);
    state.append(event_at(thread, 1)).unwrap();
    state
        .append(child_completed(thread, "child:duplicate"))
        .unwrap();
    state
        .append(RuntimeEvent::ChildCompletionAcknowledged {
            seq: 0,
            thread_id: thread,
            turn_id: ark_common::TurnId::new(),
            timestamp: Utc::now(),
            notification_id: "child:duplicate".to_string(),
        })
        .unwrap();
    state
        .append(child_completed(thread, "child:duplicate"))
        .unwrap();
    state.append(event_at(thread, 1)).unwrap();
    state.append(event_at(thread, 1)).unwrap();

    let compacted_to = state.compact(3).unwrap();

    assert_eq!(compacted_to, Some(2));
    assert!(state.events().iter().any(|event| {
        matches!(
            event,
            RuntimeEvent::ChildCompletionAcknowledged { notification_id, .. }
                if notification_id == "child:duplicate"
        )
    }));
}

#[test]
fn rewind_events_write_failure_keeps_memory_and_aborts_on_next_append() {
    let dir = TempDir::new("rewind-events-failure");
    let thread = ThreadId::new();
    let mut state = state(&dir, thread);
    for _ in 0..4 {
        state.append(event_at(thread, 0)).unwrap();
    }
    let paths = state.paths().clone();
    crate::store::rewind_tx::fail_once(&paths, crate::store::rewind_tx::TestStage::EventsWrite);

    assert!(state.rewind(3, None).is_err());
    assert_eq!(
        state
            .events()
            .iter()
            .map(RuntimeEvent::seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert!(paths.rewind_pending_json().exists());

    let appended = state.append(event_at(thread, 0)).unwrap();
    assert_eq!(appended.seq(), 5);
    assert_eq!(
        state
            .events()
            .iter()
            .map(RuntimeEvent::seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert!(!paths.rewind_pending_json().exists());
}

#[test]
fn rewind_derived_failure_recovers_all_files_before_next_append() {
    let dir = TempDir::new("rewind-derived-failure");
    let thread = ThreadId::new();
    let turn_a = TurnId::new();
    let turn_b = TurnId::new();
    let mut state = state(&dir, thread);
    state.append(terminal(thread, turn_a)).unwrap();
    state.append(usage(thread, turn_a, 3)).unwrap();
    state.append(terminal(thread, turn_b)).unwrap();
    state.append(usage(thread, turn_b, 7)).unwrap();
    let paths = state.paths().clone();
    let mut subscription = state.snapshot_and_subscribe(0).unwrap();
    crate::store::rewind_tx::fail_once(&paths, crate::store::rewind_tx::TestStage::DerivedRebuild);

    assert!(state.rewind(3, Some(turn_a)).is_err());
    assert_eq!(state.events().len(), 4, "failed call keeps old memory");
    assert!(paths.rewind_pending_json().exists());

    let appended = state.append(event_at(thread, 0)).unwrap();
    assert_eq!(appended.seq(), 6);
    assert_eq!(
        state
            .events()
            .iter()
            .map(RuntimeEvent::seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 5, 6]
    );
    assert_eq!(
        crate::store::items_log::thread_usage_blocking(&paths)
            .unwrap()
            .total_tokens,
        3
    );
    let session =
        crate::store::fs_util::read_json_opt::<SessionState>(&paths.session_json()).unwrap();
    assert_eq!(session.last_turn_id, Some(turn_a));
    assert!(state.rewind_record(Some(3), None).is_some());
    assert!(matches!(
        subscription.live.try_recv(),
        Ok(RuntimeEvent::ThreadRewound { seq: 5, .. })
    ));
    assert!(matches!(
        subscription.live.try_recv(),
        Ok(RuntimeEvent::Heartbeat { seq: 6, .. })
    ));
    assert!(!paths.rewind_pending_json().exists());
}

#[test]
fn pending_rewind_recovers_before_and_after_event_replacement_on_open() {
    for (tag, stage, expected) in [
        (
            "before",
            crate::store::rewind_tx::TestStage::EventsWrite,
            vec![1, 2, 3, 4],
        ),
        (
            "after",
            crate::store::rewind_tx::TestStage::DerivedRebuild,
            vec![1, 2, 5],
        ),
    ] {
        let dir = TempDir::new(tag);
        let thread = ThreadId::new();
        let mut original = state(&dir, thread);
        for _ in 0..4 {
            original.append(event_at(thread, 0)).unwrap();
        }
        let paths = original.paths().clone();
        crate::store::rewind_tx::fail_once(&paths, stage);
        assert!(original.rewind(3, None).is_err());
        drop(original);

        let reopened = state(&dir, thread);
        assert_eq!(
            reopened
                .events()
                .iter()
                .map(RuntimeEvent::seq)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(!paths.rewind_pending_json().exists());
    }
}

fn terminal(thread: ThreadId, turn: TurnId) -> RuntimeEvent {
    RuntimeEvent::TurnCompleted {
        seq: 0,
        thread_id: thread,
        turn_id: turn,
        timestamp: Utc::now(),
    }
}

fn usage(thread: ThreadId, turn: TurnId, tokens: u64) -> RuntimeEvent {
    RuntimeEvent::Usage {
        seq: 0,
        thread_id: thread,
        turn_id: turn,
        timestamp: Utc::now(),
        usage: ark_common::Usage::new(tokens, 0),
    }
}

#[test]
fn append_fsync_skips_deltas_and_respects_the_limit() {
    let mut limits = StoreLimits::for_tests();
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let delta = text_delta(thread, turn, ItemId::new(), 1, "chunk");
    let key_point = event_at(thread, 2);

    // With fsync enabled, streaming deltas skip the flush (a crash loses at
    // most the torn tail and the projection replays from the log); every
    // other event keeps the flush.
    limits.fsync_on_append = true;
    assert!(
        !super::append_fsync(&limits, &delta),
        "delta must skip fsync"
    );
    assert!(super::append_fsync(&limits, &key_point));

    // The limit still wins: turning it off disables fsync entirely.
    limits.fsync_on_append = false;
    assert!(!super::append_fsync(&limits, &delta));
    assert!(!super::append_fsync(&limits, &key_point));

    // Derived append durability belongs to the dirty-marker transaction,
    // not the event-log tuning. Only the file actually appended is synced.
    let item = RuntimeEvent::ToolCallStarted {
        seq: 3,
        thread_id: thread,
        turn_id: turn,
        timestamp: Utc::now(),
        call_id: "call".into(),
        tool: "read".into(),
        arguments: serde_json::json!({}),
    };
    let usage = RuntimeEvent::Usage {
        seq: 4,
        thread_id: thread,
        turn_id: turn,
        timestamp: Utc::now(),
        usage: ark_common::Usage::new(1, 1),
    };
    assert!(!super::derived_append_fsync(
        &delta,
        super::DerivedAppend::Item
    ));
    assert!(super::derived_append_fsync(
        &item,
        super::DerivedAppend::Item
    ));
    assert!(!super::derived_append_fsync(
        &key_point,
        super::DerivedAppend::Item
    ));
    assert!(super::derived_append_fsync(
        &usage,
        super::DerivedAppend::Usage
    ));
    assert!(!super::derived_append_fsync(
        &item,
        super::DerivedAppend::Usage
    ));
}
