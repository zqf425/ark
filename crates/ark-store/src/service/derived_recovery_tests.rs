use crate::config::{StoreConfig, StoreLimits};
use crate::domain::diagnostics::{DiagnosticKind, MemoryDiagnostics};
use crate::domain::items::ItemProjector;
use crate::domain::layout::{DataPaths, ThreadPaths};
use crate::domain::session_state::SessionState;
use crate::domain::thread_file::ThreadFile;
use crate::service::store::FileSessionStore;
use crate::store::event_log::{encode_event, encode_line};
use crate::store::fs_util::{ensure_dir, write_json_atomic};
use crate::store::index::{save_snapshot, IndexEntry, ThreadIndex};
use crate::store::items_log::{
    read_item_rows_blocking, read_items_blocking, thread_usage_blocking,
    write_items_records_blocking, ItemRow, UsageRecord,
};
use crate::store::tests::TempDir;
use crate::store::writer_state::WriterState;
use ark_common::{ArkErrorCode, ChildId, ItemId, ThreadId, TurnId, Usage};
use ark_contract::RuntimeEvent;
use ark_ports::{ChildRunRecord, SessionStore};
use std::sync::Arc;

fn events(thread: ThreadId) -> Vec<RuntimeEvent> {
    let turn = TurnId::new();
    let item = ItemId::new();
    let now = chrono::Utc::now();
    vec![
        RuntimeEvent::TurnStarted {
            seq: 1,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        },
        RuntimeEvent::AssistantTextDelta {
            seq: 2,
            thread_id: thread,
            turn_id: turn,
            item_id: item,
            timestamp: now,
            delta: "first".into(),
        },
        RuntimeEvent::AssistantTextDelta {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            item_id: item,
            timestamp: now,
            delta: " second".into(),
        },
        RuntimeEvent::Usage {
            seq: 4,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            usage: Usage::new(10, 5),
        },
        RuntimeEvent::Usage {
            seq: 5,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            usage: Usage::new(20, 5),
        },
        RuntimeEvent::TurnCompleted {
            seq: 6,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
        },
    ]
}

fn paths(dir: &TempDir, thread: ThreadId) -> ThreadPaths {
    DataPaths::new(dir.path()).thread(&thread)
}

fn write_events(paths: &ThreadPaths, events: &[RuntimeEvent]) {
    ensure_dir(paths.dir()).unwrap();
    let body = events
        .iter()
        .map(|event| encode_event(event).unwrap())
        .collect::<String>();
    std::fs::write(paths.events_jsonl(), body).unwrap();
}

fn write_thread(paths: &ThreadPaths, thread: ThreadId) {
    ensure_dir(paths.dir()).unwrap();
    write_json_atomic(&paths.thread_json(), &ThreadFile::new(thread, "recovery")).unwrap();
}

fn write_index(dir: &TempDir, thread: ThreadId, indexed_seq: u64) {
    let data = DataPaths::new(dir.path());
    let mut entry = IndexEntry::new(
        thread,
        "recovery",
        data.thread_rel(&thread),
        chrono::Utc::now(),
    );
    entry.indexed_seq = indexed_seq;
    let mut index = ThreadIndex::new();
    index.insert(entry);
    save_snapshot(&mut index, &data.index_file()).unwrap();
}

fn write_stale(paths: &ThreadPaths, events: &[RuntimeEvent]) {
    crate::store::items_log::rewrite_items_from_events_blocking(paths, &events[..2]).unwrap();
    let RuntimeEvent::Usage {
        turn_id,
        timestamp,
        usage,
        ..
    } = &events[3]
    else {
        unreachable!()
    };
    let usage = UsageRecord {
        seq: 4,
        turn_id: Some(*turn_id),
        usage: usage.clone(),
        at: *timestamp,
    };
    std::fs::write(paths.usage_jsonl(), encode_line(&usage).unwrap()).unwrap();
    write_json_atomic(&paths.session_json(), &SessionState::replay(&events[..1])).unwrap();
}

fn prepare(dir: &TempDir, events: &[RuntimeEvent], indexed_seq: u64) -> ThreadPaths {
    let thread = events[0].thread_id();
    let paths = paths(dir, thread);
    write_thread(&paths, thread);
    write_events(&paths, events);
    write_stale(&paths, events);
    write_index(dir, thread, indexed_seq);
    paths
}

fn derived_bytes(paths: &ThreadPaths) -> [Vec<u8>; 3] {
    [
        std::fs::read(paths.items_jsonl()).unwrap(),
        std::fs::read(paths.usage_jsonl()).unwrap(),
        std::fs::read(paths.session_json()).unwrap(),
    ]
}

fn open(dir: &TempDir) -> (FileSessionStore, Arc<MemoryDiagnostics>) {
    let diagnostics = Arc::new(MemoryDiagnostics::new());
    let store = FileSessionStore::with_diagnostics(
        StoreConfig::new(dir.path()).with_limits(StoreLimits::for_tests()),
        diagnostics.clone(),
    )
    .unwrap();
    (store, diagnostics)
}

fn assert_open_preserves(dir: &TempDir, paths: &ThreadPaths) {
    let before = derived_bytes(paths);
    let (store, _) = open(dir);
    assert_eq!(derived_bytes(paths), before);
    drop(store);
}

#[test]
fn startup_repairs_three_lagging_derived_files_when_index_matches() {
    let dir = TempDir::new("derived-startup-repair");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 6);
    let before = derived_bytes(&paths);

    let (store, diagnostics) = open(&dir);

    assert_ne!(derived_bytes(&paths), before);
    assert_eq!(
        read_items_blocking(&paths).unwrap()[0].text.as_deref(),
        Some("first second")
    );
    assert_eq!(thread_usage_blocking(&paths).unwrap().total_tokens, 40);
    let expected_session = SessionState::replay(&events);
    let actual_session =
        crate::store::fs_util::read_json_opt::<SessionState>(&paths.session_json()).unwrap();
    assert_eq!(actual_session.status, expected_session.status);
    assert_eq!(actual_session.last_turn_id, expected_session.last_turn_id);
    assert_eq!(actual_session.started_at, expected_session.started_at);
    assert_eq!(actual_session.ended_at, expected_session.ended_at);
    assert_eq!(actual_session.error, expected_session.error);
    assert_eq!(
        diagnostics
            .kinds()
            .iter()
            .filter(|kind| **kind == DiagnosticKind::DerivedRebuilt)
            .count(),
        3
    );
    drop(store);
}

#[test]
fn startup_skips_when_index_lags_without_dirty_marker() {
    let dir = TempDir::new("derived-index-behind-clean");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 5);

    assert_open_preserves(&dir, &paths);
}

#[test]
fn startup_skips_dirty_thread_when_index_row_is_missing() {
    let dir = TempDir::new("derived-index-missing-dirty");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 6);
    std::fs::remove_file(DataPaths::new(dir.path()).index_file()).unwrap();
    std::fs::write(paths.derived_dirty_json(), b"{}").unwrap();

    assert_open_preserves(&dir, &paths);
    assert!(paths.derived_dirty_json().exists());
}

#[test]
fn startup_repairs_lagging_index_with_dirty_marker_and_clears_it() {
    let dir = TempDir::new("derived-index-behind-dirty");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 5);
    std::fs::write(paths.derived_dirty_json(), br#"{"planned_seq":5}"#).unwrap();
    let before = derived_bytes(&paths);

    let (store, diagnostics) = open(&dir);

    assert_ne!(derived_bytes(&paths), before);
    assert!(!paths.derived_dirty_json().exists());
    assert_eq!(
        diagnostics
            .kinds()
            .iter()
            .filter(|kind| **kind == DiagnosticKind::DerivedRebuilt)
            .count(),
        2
    );
    drop(store);
}

#[test]
fn startup_clears_marker_when_planned_event_never_reached_the_log() {
    let dir = TempDir::new("derived-uncommitted-planned-event");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 4);
    write_events(&paths, &events[..4]);
    std::fs::write(paths.derived_dirty_json(), br#"{"planned_seq":5}"#).unwrap();
    let before = derived_bytes(&paths);

    let (store, _) = open(&dir);

    assert_eq!(derived_bytes(&paths), before);
    assert!(!paths.derived_dirty_json().exists());
    drop(store);
}

#[test]
fn startup_preserves_uncommitted_marker_when_sidecar_exists() {
    let dir = TempDir::new("derived-uncommitted-sidecar");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 4);
    write_events(&paths, &events[..4]);
    std::fs::write(paths.compacted_jsonl(), b"").unwrap();
    std::fs::write(paths.derived_dirty_json(), br#"{"planned_seq":5}"#).unwrap();

    assert_open_preserves(&dir, &paths);
    assert!(paths.derived_dirty_json().exists());
}

#[test]
fn startup_skips_dirty_marker_when_index_is_ahead_of_the_log() {
    let dir = TempDir::new("derived-index-ahead");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 7);
    std::fs::write(paths.derived_dirty_json(), b"{}").unwrap();

    assert_open_preserves(&dir, &paths);
    assert!(paths.derived_dirty_json().exists());
}

#[test]
fn startup_skips_clean_tail_truncation_and_internal_gap() {
    let complete = events(ThreadId::new());
    for (tag, selected, index_seq) in [
        (
            "tail",
            vec![
                complete[0].clone(),
                complete[1].clone(),
                complete[2].clone(),
            ],
            6,
        ),
        ("gap", vec![complete[0].clone(), complete[2].clone()], 3),
    ] {
        let dir = TempDir::new(tag);
        let paths = prepare(&dir, &complete, index_seq);
        write_events(&paths, &selected);
        assert_open_preserves(&dir, &paths);
    }
}

#[test]
fn startup_skips_dirty_thread_with_empty_or_damaged_compaction_sidecar() {
    for sidecar in ["", "not-json\n"] {
        let dir = TempDir::new("derived-sidecar");
        let thread = ThreadId::new();
        let events = events(thread);
        let paths = prepare(&dir, &events, 6);
        std::fs::write(paths.compacted_jsonl(), sidecar).unwrap();
        std::fs::write(paths.derived_dirty_json(), b"not-json").unwrap();

        assert_open_preserves(&dir, &paths);
        assert!(paths.derived_dirty_json().exists());
    }
}

#[test]
fn startup_skips_dirty_thread_with_a_sequence_gap() {
    let dir = TempDir::new("derived-dirty-gap");
    let thread = ThreadId::new();
    let complete = events(thread);
    let paths = prepare(&dir, &complete, 3);
    write_events(&paths, &[complete[0].clone(), complete[2].clone()]);
    std::fs::write(paths.derived_dirty_json(), br#"{"planned_seq":4}"#).unwrap();

    assert_open_preserves(&dir, &paths);
    assert!(paths.derived_dirty_json().exists());
}

#[test]
fn startup_keeps_dirty_marker_when_one_repair_write_fails() {
    let dir = TempDir::new("derived-repair-write-failure");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 5);
    std::fs::write(paths.derived_dirty_json(), br#"{"planned_seq":5}"#).unwrap();
    std::fs::remove_file(paths.usage_jsonl()).unwrap();
    std::fs::create_dir(paths.usage_jsonl()).unwrap();

    let (store, diagnostics) = open(&dir);

    assert!(paths.derived_dirty_json().exists());
    assert!(diagnostics.has(DiagnosticKind::DerivedWriteFailed));
    drop(store);
}

#[test]
fn malformed_dirty_marker_is_preserved_without_rewriting() {
    let dir = TempDir::new("derived-malformed-marker");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 5);
    std::fs::write(paths.derived_dirty_json(), b"not-json").unwrap();

    assert_open_preserves(&dir, &paths);
    assert_eq!(
        std::fs::read(paths.derived_dirty_json()).unwrap(),
        b"not-json"
    );
}

#[test]
fn compacted_dirty_thread_recovers_from_checkpoints_and_suffix() {
    let dir = TempDir::new("derived-compacted-checkpoint");
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let now = chrono::Utc::now();
    let mut prefix_item = ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::Notice)
        .with_text("prefix");
    let mut suffix_item =
        ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::AssistantMessage)
            .with_text("suffix");
    let suffix_item_id = ItemId::new();
    prefix_item.item_id = ItemId::new();
    suffix_item.item_id = suffix_item_id;
    let all = [
        crate::store::tests::turn_started(thread, turn, 1),
        RuntimeEvent::ItemCreated {
            seq: 2,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            item: prefix_item.clone(),
        },
        RuntimeEvent::Usage {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            usage: Usage::new(4, 1),
        },
        RuntimeEvent::Error {
            seq: 4,
            thread_id: thread,
            timestamp: now,
            code: ark_common::ArkErrorCode::ModelRequestFailed,
            message: "suffix failure".into(),
        },
        RuntimeEvent::ItemCreated {
            seq: 5,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            item: suffix_item,
        },
        RuntimeEvent::AssistantTextDelta {
            seq: 6,
            thread_id: thread,
            turn_id: turn,
            item_id: suffix_item_id,
            timestamp: now,
            delta: " tail".into(),
        },
        RuntimeEvent::Usage {
            seq: 7,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            usage: Usage::new(8, 2),
        },
        RuntimeEvent::TurnCompleted {
            seq: 8,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
        },
    ];
    let paths = paths(&dir, thread);
    write_thread(&paths, thread);
    write_events(&paths, &all[3..]);
    crate::store::items_log::rewrite_items_from_events_blocking(&paths, &all[..3]).unwrap();
    let prefix_usage = UsageRecord {
        seq: 3,
        turn_id: Some(turn),
        usage: Usage::new(4, 1),
        at: now,
    };
    std::fs::write(paths.usage_jsonl(), encode_line(&prefix_usage).unwrap()).unwrap();
    let pre_session = SessionState::replay(&all[..3]);
    write_json_atomic(&paths.session_json(), &pre_session).unwrap();
    std::fs::write(
        paths.compacted_jsonl(),
        format!(
            "{}\n",
            crate::domain::compaction::CompactionMarker::new(1, 3, 4, "test").to_line()
        ),
    )
    .unwrap();
    crate::store::derived_dirty::mark_if_clean(&paths, 4, Some(turn), &pre_session).unwrap();
    write_index(&dir, thread, 8);

    let (store, _) = open(&dir);

    let items = read_items_blocking(&paths).unwrap();
    assert_eq!(items.len(), 3, "the compacted prefix item must survive");
    assert!(items.iter().any(|item| item.item_id == prefix_item.item_id));
    let error = items
        .iter()
        .find(|item| item.text.as_deref() == Some("suffix failure"))
        .expect("dirty suffix error is recovered");
    assert_eq!(error.turn_id, turn);
    assert_eq!(
        items
            .iter()
            .find(|item| item.item_id == suffix_item_id)
            .and_then(|item| item.text.as_deref()),
        Some("suffix tail")
    );
    assert_eq!(thread_usage_blocking(&paths).unwrap().total_tokens, 15);
    let session =
        crate::store::fs_util::read_json_opt::<SessionState>(&paths.session_json()).unwrap();
    assert_eq!(
        session.status,
        crate::domain::session_state::SessionStatus::Completed
    );
    assert!(!paths.derived_dirty_json().exists());
    drop(store);
}

#[test]
fn child_dirty_thread_recovers_without_a_global_index_row() {
    let dir = TempDir::new("derived-child-checkpoint");
    let parent = ThreadId::new();
    let child = ThreadId::new();
    let parent_paths = paths(&dir, parent);
    write_thread(&parent_paths, parent);
    write_events(&parent_paths, &[]);
    write_index(&dir, parent, 0);
    let child_paths = ThreadPaths::new(parent_paths.children_dir().join(child.to_string()));
    ensure_dir(child_paths.dir()).unwrap();
    let mut child_file = ThreadFile::new(child, "child");
    child_file.parent_thread_id = Some(parent);
    write_json_atomic(&child_paths.thread_json(), &child_file).unwrap();
    let child_events = events(child);
    write_events(&child_paths, &child_events);
    write_stale(&child_paths, &child_events);
    std::fs::write(child_paths.derived_dirty_json(), br#"{"planned_seq":3}"#).unwrap();

    let (store, _) = open(&dir);

    assert_eq!(
        read_items_blocking(&child_paths).unwrap()[0]
            .text
            .as_deref(),
        Some("first second")
    );
    assert_eq!(
        thread_usage_blocking(&child_paths).unwrap().total_tokens,
        40
    );
    let session =
        crate::store::fs_util::read_json_opt::<SessionState>(&child_paths.session_json()).unwrap();
    assert_eq!(
        session.status,
        crate::domain::session_state::SessionStatus::Completed
    );
    assert!(!child_paths.derived_dirty_json().exists());
    drop(store);
}

#[tokio::test]
async fn child_pending_rewind_is_recovered_before_read_only_access() {
    let dir = TempDir::new("child-pending-rewind-startup");
    let (store, _) = open(&dir);
    let parent = store.create_thread("parent").await.unwrap().thread_id;
    let child = ChildId::new();
    let child_thread = ThreadId::from_uuid(child.as_uuid());
    store
        .create_child_run(parent, child, "rewind child")
        .await
        .unwrap();
    store
        .append_child_run(ChildRunRecord::queued(
            child,
            parent,
            TurnId::new(),
            "rewind child",
            chrono::Utc::now(),
        ))
        .await
        .unwrap();
    let child_paths = store.resolve_paths(child_thread);
    let retained_turn = TurnId::new();
    let removed_turn = TurnId::new();

    for event in [
        RuntimeEvent::Error {
            seq: 0,
            thread_id: child_thread,
            timestamp: chrono::Utc::now(),
            code: ArkErrorCode::ModelRequestFailed,
            message: "retained child item".into(),
        },
        RuntimeEvent::Usage {
            seq: 0,
            thread_id: child_thread,
            turn_id: retained_turn,
            timestamp: chrono::Utc::now(),
            usage: Usage::new(3, 0),
        },
        RuntimeEvent::Error {
            seq: 0,
            thread_id: child_thread,
            timestamp: chrono::Utc::now(),
            code: ArkErrorCode::ModelRequestFailed,
            message: "removed child item".into(),
        },
        RuntimeEvent::Usage {
            seq: 0,
            thread_id: child_thread,
            turn_id: removed_turn,
            timestamp: chrono::Utc::now(),
            usage: Usage::new(7, 0),
        },
    ] {
        store.append_event(child_thread, event).await.unwrap();
    }
    crate::store::rewind_tx::fail_once(
        &child_paths,
        crate::store::rewind_tx::TestStage::DerivedRebuild,
    );

    assert!(store
        .rewind_thread(child_thread, 3, Some(retained_turn))
        .await
        .is_err());
    assert!(child_paths.rewind_pending_json().exists());
    drop(store);

    let (reopened, _) = open(&dir);
    assert!(!child_paths.rewind_pending_json().exists());
    assert_eq!(
        reopened.resolve_paths(child_thread).dir(),
        child_paths.dir()
    );

    let items = reopened.read_items(child_thread).await.unwrap();
    assert!(items
        .iter()
        .any(|item| item.text.as_deref() == Some("retained child item")));
    assert!(!items
        .iter()
        .any(|item| item.text.as_deref() == Some("removed child item")));
    assert_eq!(thread_usage_blocking(&child_paths).unwrap().total_tokens, 3);

    let stored = reopened
        .append_event(
            child_thread,
            RuntimeEvent::Error {
                seq: 0,
                thread_id: child_thread,
                timestamp: chrono::Utc::now(),
                code: ArkErrorCode::ModelRequestFailed,
                message: "post-restart child item".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(stored.seq(), 6);
    assert!(!reopened.thread_paths(child_thread).dir().exists());
    assert!(reopened
        .read_items(child_thread)
        .await
        .unwrap()
        .iter()
        .any(|item| item.text.as_deref() == Some("post-restart child item")));
    drop(reopened);
}

#[test]
fn startup_rewrites_a_lagging_delta_watermark() {
    let dir = TempDir::new("derived-delta-watermark");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 6);

    let (store, _) = open(&dir);

    let rows = read_item_rows_blocking(&paths);
    let ItemRow::Watermarked(record) = &rows[0] else {
        panic!("startup repair must emit a watermarked row");
    };
    assert_eq!(record.watermark_seq, 3);
    assert_eq!(record.item.text.as_deref(), Some("first second"));
    drop(store);
}

#[test]
fn startup_rewrites_a_lagging_non_delta_watermark() {
    let dir = TempDir::new("derived-non-delta-watermark");
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let now = chrono::Utc::now();
    let mut old =
        ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::Notice).with_text("old");
    let mut new = old.clone().with_text("new");
    let item_id = ItemId::new();
    old.item_id = item_id;
    new.item_id = item_id;
    let events = vec![
        crate::store::tests::turn_started(thread, turn, 1),
        RuntimeEvent::ItemCreated {
            seq: 2,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            item: old,
        },
        RuntimeEvent::ItemUpdated {
            seq: 3,
            thread_id: thread,
            turn_id: turn,
            timestamp: now,
            item: new,
        },
    ];
    let paths = paths(&dir, thread);
    write_thread(&paths, thread);
    write_events(&paths, &events);
    write_index(&dir, thread, 3);
    crate::store::items_log::rewrite_items_from_events_blocking(&paths, &events[..2]).unwrap();
    write_json_atomic(&paths.session_json(), &SessionState::replay(&events)).unwrap();

    let (store, _) = open(&dir);

    let ItemRow::Watermarked(record) = &read_item_rows_blocking(&paths)[0] else {
        panic!("startup repair must emit a watermarked row");
    };
    assert_eq!(record.watermark_seq, 3);
    assert_eq!(record.item.text.as_deref(), Some("new"));
    drop(store);
}

#[test]
fn canonical_item_history_with_the_same_final_snapshot_is_not_rewritten() {
    let dir = TempDir::new("derived-canonical-items");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 6);
    let mut first = ItemProjector::new();
    first.apply(&events[1]);
    let mut final_projection = ItemProjector::new();
    for event in &events {
        final_projection.apply(event);
    }
    let mut history = first.snapshot_records();
    history.extend(final_projection.snapshot_records());
    write_items_records_blocking(&paths, &history).unwrap();
    let before = std::fs::read(paths.items_jsonl()).unwrap();

    let (store, diagnostics) = open(&dir);

    assert_eq!(std::fs::read(paths.items_jsonl()).unwrap(), before);
    assert_eq!(
        diagnostics
            .kinds()
            .iter()
            .filter(|kind| **kind == DiagnosticKind::DerivedRebuilt)
            .count(),
        2
    );
    drop(store);
}

#[test]
fn empty_events_preserve_cache_reads_and_bytes() {
    let dir = TempDir::new("derived-empty-events");
    let thread = ThreadId::new();
    let paths = paths(&dir, thread);
    write_thread(&paths, thread);
    std::fs::write(paths.events_jsonl(), []).unwrap();
    let item = ark_contract::ThreadItem::new(TurnId::new(), ark_contract::item::ItemKind::Notice)
        .with_text("surviving");
    std::fs::write(paths.items_jsonl(), encode_line(&item).unwrap()).unwrap();
    let usage = UsageRecord::from_event(9, TurnId::new(), Usage::new(7, 3));
    std::fs::write(paths.usage_jsonl(), encode_line(&usage).unwrap()).unwrap();
    std::fs::write(paths.session_json(), b"legacy-session-bytes").unwrap();
    write_index(&dir, thread, 0);
    let before = derived_bytes(&paths);

    let (store, _) = open(&dir);

    assert_eq!(derived_bytes(&paths), before);
    assert_eq!(read_items_blocking(&paths).unwrap(), vec![item]);
    assert_eq!(thread_usage_blocking(&paths).unwrap().total_tokens, 10);
    drop(store);
}

#[test]
fn malformed_thread_does_not_fail_startup_or_overwrite_caches() {
    let dir = TempDir::new("derived-malformed-events");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = prepare(&dir, &events, 6);
    let mut body = encode_event(&events[0]).unwrap();
    body.push_str("not-json\n");
    body.push_str(&encode_event(&events[1]).unwrap());
    std::fs::write(paths.events_jsonl(), body).unwrap();
    let before = derived_bytes(&paths);

    let (store, diagnostics) = open(&dir);

    assert_eq!(derived_bytes(&paths), before);
    let diagnostic = diagnostics
        .entries()
        .into_iter()
        .find(|entry| entry.kind == DiagnosticKind::DerivedWriteFailed)
        .unwrap();
    assert!(diagnostic
        .detail
        .contains("skipped derived repair for thread"));
    drop(store);
}

#[test]
fn bare_writer_open_does_not_rewrite_stale_caches() {
    let dir = TempDir::new("derived-writer-open");
    let thread = ThreadId::new();
    let events = events(thread);
    let paths = paths(&dir, thread);
    write_events(&paths, &events);
    write_stale(&paths, &events);
    let before = derived_bytes(&paths);

    WriterState::open(
        thread,
        paths.clone(),
        StoreLimits::for_tests(),
        Arc::new(MemoryDiagnostics::new()),
    )
    .unwrap();

    assert_eq!(derived_bytes(&paths), before);
}
