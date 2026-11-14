use crate::config::StoreLimits;
use crate::domain::diagnostics::{DiagnosticKind, MemoryDiagnostics};
use crate::domain::layout::{DataPaths, ThreadPaths};
use crate::domain::session_state::SessionState;
use crate::store::fs_util::ensure_dir;
use crate::store::items_log::{read_items_blocking, thread_usage_blocking};
use crate::store::tests::TempDir;
use crate::store::writer_state::WriterState;
use ark_common::{ItemId, ThreadId, TurnId, Usage};
use ark_contract::RuntimeEvent;
use std::sync::Arc;

fn paths(dir: &TempDir, thread: ThreadId) -> ThreadPaths {
    DataPaths::new(dir.path()).thread(&thread)
}

fn open_writer(
    paths: ThreadPaths,
    thread: ThreadId,
    diagnostics: Arc<MemoryDiagnostics>,
) -> WriterState {
    WriterState::open(thread, paths, StoreLimits::for_tests(), diagnostics).unwrap()
}

#[test]
fn successful_derived_append_clears_its_dirty_marker() {
    let dir = TempDir::new("derived-append-clean");
    let thread = ThreadId::new();
    let paths = paths(&dir, thread);
    let mut writer = open_writer(paths.clone(), thread, Arc::new(MemoryDiagnostics::new()));

    writer
        .append(crate::store::tests::turn_started(thread, TurnId::new(), 0))
        .unwrap();

    assert!(!paths.derived_dirty_json().exists());
}

#[test]
fn existing_dirty_marker_survives_a_later_successful_append() {
    let dir = TempDir::new("derived-append-existing-dirty");
    let thread = ThreadId::new();
    let paths = paths(&dir, thread);
    ensure_dir(paths.dir()).unwrap();
    std::fs::write(paths.derived_dirty_json(), b"older-failure").unwrap();
    let mut writer = open_writer(paths.clone(), thread, Arc::new(MemoryDiagnostics::new()));

    writer
        .append(crate::store::tests::turn_started(thread, TurnId::new(), 0))
        .unwrap();

    assert_eq!(
        std::fs::read(paths.derived_dirty_json()).unwrap(),
        b"older-failure"
    );
}

#[test]
fn failed_derived_append_keeps_marker_and_records_diagnostic() {
    let dir = TempDir::new("derived-append-write-failure");
    let thread = ThreadId::new();
    let paths = paths(&dir, thread);
    let diagnostics = Arc::new(MemoryDiagnostics::new());
    let mut writer = open_writer(paths.clone(), thread, diagnostics.clone());
    std::fs::create_dir_all(paths.session_json()).unwrap();

    let stored = writer
        .append(crate::store::tests::turn_started(thread, TurnId::new(), 0))
        .unwrap();

    assert_eq!(stored.seq(), 1);
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(paths.derived_dirty_json()).unwrap()).unwrap();
    assert_eq!(marker["planned_seq"], 1);
    assert!(diagnostics.has(DiagnosticKind::DerivedWriteFailed));
}

#[test]
fn delta_append_neither_creates_nor_clears_dirty_marker() {
    let dir = TempDir::new("derived-delta-marker");
    let thread = ThreadId::new();
    let paths = paths(&dir, thread);
    let mut writer = open_writer(paths.clone(), thread, Arc::new(MemoryDiagnostics::new()));
    let delta = crate::store::tests::text_delta(thread, TurnId::new(), ItemId::new(), 0, "chunk");

    writer.append(delta.clone()).unwrap();
    assert!(!paths.derived_dirty_json().exists());
    std::fs::write(paths.derived_dirty_json(), b"older-failure").unwrap();
    writer.append(delta).unwrap();
    assert_eq!(
        std::fs::read(paths.derived_dirty_json()).unwrap(),
        b"older-failure"
    );
}

fn prepare_compacted_writer_checkpoint(
    paths: &ThreadPaths,
    thread: ThreadId,
    turn: TurnId,
) -> chrono::DateTime<chrono::Utc> {
    let started_at = chrono::Utc::now();
    let mut writer = open_writer(paths.clone(), thread, Arc::new(MemoryDiagnostics::new()));
    writer
        .append(RuntimeEvent::TurnStarted {
            auto_recovery: 0,
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: started_at,
            auto_continuation: false,
            attachment_ids: Vec::new(),
            attachment_outcomes: Vec::new(),
        })
        .unwrap();
    writer
        .append(RuntimeEvent::ToolCallStarted {
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: started_at,
            call_id: "checkpoint-call".into(),
            tool: "read".into(),
            arguments: serde_json::json!({"path": "prefix.txt"}),
        })
        .unwrap();
    writer
        .append(RuntimeEvent::Usage {
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: started_at,
            usage: Usage::new(3, 2),
        })
        .unwrap();
    writer
        .append(crate::store::tests::event_at(thread, 0))
        .unwrap();
    assert_eq!(writer.compact(1).unwrap(), Some(3));
    started_at
}

#[test]
fn compacted_writer_restart_preserves_item_and_session_checkpoints() {
    let dir = TempDir::new("compacted-writer-checkpoints");
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let paths = paths(&dir, thread);
    let started_at = prepare_compacted_writer_checkpoint(&paths, thread, turn);
    let mut writer = open_writer(paths.clone(), thread, Arc::new(MemoryDiagnostics::new()));
    writer
        .append(RuntimeEvent::Error {
            seq: 0,
            thread_id: thread,
            timestamp: chrono::Utc::now(),
            code: ark_common::ArkErrorCode::ModelRequestFailed,
            message: "retained suffix failure".into(),
        })
        .unwrap();
    writer
        .append(RuntimeEvent::ToolCallFinished {
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: chrono::Utc::now(),
            call_id: "checkpoint-call".into(),
            tool: "read".into(),
            ok: true,
            duration_ms: 7,
            output: "done".into(),
            detail: None,
        })
        .unwrap();
    writer
        .append(RuntimeEvent::TurnCompleted {
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: chrono::Utc::now(),
        })
        .unwrap();

    let items = read_items_blocking(&paths).unwrap();
    let tool = items
        .iter()
        .find_map(|item| item.tool.as_ref())
        .expect("tool checkpoint survives");
    assert_eq!(
        tool.arguments,
        Some(serde_json::json!({"path": "prefix.txt"}))
    );
    assert_eq!(tool.output.as_deref(), Some("done"));
    let error = items
        .iter()
        .find(|item| item.text.as_deref() == Some("retained suffix failure"))
        .expect("error appended after restart is projected");
    assert_eq!(error.turn_id, turn);
    let session =
        crate::store::fs_util::read_json_opt::<SessionState>(&paths.session_json()).unwrap();
    assert_eq!(session.started_at, Some(started_at));
    assert_eq!(session.last_turn_id, Some(turn));
}

#[cfg(unix)]
#[test]
fn dirty_compacted_reads_merge_suffix_after_derived_append_failures() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("compacted-dirty-live-read");
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let paths = paths(&dir, thread);
    prepare_compacted_writer_checkpoint(&paths, thread, turn);
    let mut writer = open_writer(paths.clone(), thread, Arc::new(MemoryDiagnostics::new()));
    for path in [paths.items_jsonl(), paths.usage_jsonl()] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444)).unwrap();
    }
    let mut item =
        ark_contract::ThreadItem::new(turn, ark_contract::item::ItemKind::AssistantMessage)
            .with_text("new suffix item");
    item.item_id = ItemId::new();
    writer
        .append(RuntimeEvent::ItemCreated {
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: chrono::Utc::now(),
            item: item.clone(),
        })
        .unwrap();
    writer
        .append(RuntimeEvent::Usage {
            seq: 0,
            thread_id: thread,
            turn_id: turn,
            timestamp: chrono::Utc::now(),
            usage: Usage::new(7, 3),
        })
        .unwrap();
    for path in [paths.items_jsonl(), paths.usage_jsonl()] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    assert!(paths.derived_dirty_json().exists());
    let items = read_items_blocking(&paths).unwrap();
    assert!(items.iter().any(|value| value.item_id == item.item_id));
    assert!(items.iter().any(|value| value.tool.is_some()));
    assert_eq!(thread_usage_blocking(&paths).unwrap().total_tokens, 15);
}

#[test]
fn compaction_sidecar_failure_keeps_event_bytes() {
    let dir = TempDir::new("compaction-sidecar-failure");
    let thread = ThreadId::new();
    let paths = paths(&dir, thread);
    let mut writer = open_writer(paths.clone(), thread, Arc::new(MemoryDiagnostics::new()));
    for _ in 0..3 {
        writer
            .append(crate::store::tests::event_at(thread, 0))
            .unwrap();
    }
    let before = std::fs::read(paths.events_jsonl()).unwrap();
    std::fs::create_dir(paths.compacted_jsonl()).unwrap();

    assert!(writer.compact(1).is_err());
    assert_eq!(std::fs::read(paths.events_jsonl()).unwrap(), before);
    assert_eq!(writer.events().len(), 3);
}

#[test]
fn dirty_marker_blocks_compaction_and_rewind() {
    let dir = TempDir::new("derived-destructive-gate");
    let thread = ThreadId::new();
    let paths = paths(&dir, thread);
    let mut writer = open_writer(paths.clone(), thread, Arc::new(MemoryDiagnostics::new()));
    for _ in 0..3 {
        writer
            .append(crate::store::tests::event_at(thread, 0))
            .unwrap();
    }
    std::fs::write(paths.derived_dirty_json(), b"{}").unwrap();

    let compact = writer.compact(1).unwrap_err();
    let rewind = writer.rewind(2, None).unwrap_err();

    assert_eq!(compact.code, ark_common::ArkErrorCode::Conflict);
    assert_eq!(rewind.code, ark_common::ArkErrorCode::Conflict);
    assert!(compact.message.contains("restart"));
    assert!(rewind.message.contains("restart"));
}
