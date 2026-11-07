use crate::config::{StoreConfig, StoreLimits};
use crate::domain::diagnostics::MemoryDiagnostics;
use crate::domain::session_state::SessionState;
use crate::service::store::FileSessionStore;
use crate::store::items_log::thread_usage_blocking;
use crate::store::tests::{event_at, turn_started, TempDir};
use ark_common::{ArkErrorCode, TurnId, Usage};
use ark_contract::RuntimeEvent;
use ark_ports::SessionStore;
use std::sync::Arc;

fn open(dir: &TempDir) -> FileSessionStore {
    FileSessionStore::with_diagnostics(
        StoreConfig::new(dir.path()).with_limits(StoreLimits::for_tests()),
        Arc::new(MemoryDiagnostics::new()),
    )
    .unwrap()
}

#[tokio::test]
async fn compacted_dirty_reads_use_marker_context_before_restart() {
    let dir = TempDir::new("dirty-pre-planned-turn");
    let store = open(&dir);
    let thread = store.create_thread("recovery").await.unwrap().thread_id;
    let turn_a = TurnId::new();
    let turn_b = TurnId::new();
    let paths = store.thread_paths(thread);

    store
        .append_event(thread, turn_started(thread, turn_a, 0))
        .await
        .unwrap();
    store
        .append_event(thread, usage(thread, turn_a, 2))
        .await
        .unwrap();
    store
        .append_event(thread, completed(thread, turn_a))
        .await
        .unwrap();
    store
        .append_event(thread, event_at(thread, 0))
        .await
        .unwrap();
    let writer = store.registry.writer_for(thread, &paths).unwrap();
    assert_eq!(writer.compact(1).await.unwrap(), Some(3));

    let checkpoint_items = std::fs::read(paths.items_jsonl()).unwrap();
    std::fs::remove_file(paths.items_jsonl()).unwrap();
    std::fs::create_dir(paths.items_jsonl()).unwrap();
    store
        .append_event(
            thread,
            RuntimeEvent::Error {
                seq: 0,
                thread_id: thread,
                timestamp: chrono::Utc::now(),
                code: ArkErrorCode::ModelRequestFailed,
                message: "belongs to A".into(),
            },
        )
        .await
        .unwrap();
    assert!(paths.derived_dirty_json().exists());
    std::fs::remove_dir(paths.items_jsonl()).unwrap();
    std::fs::write(paths.items_jsonl(), checkpoint_items).unwrap();

    store
        .append_event(thread, turn_started(thread, turn_b, 0))
        .await
        .unwrap();
    store
        .append_event(thread, usage(thread, turn_b, 7))
        .await
        .unwrap();
    let advanced =
        crate::store::fs_util::read_json_opt::<SessionState>(&paths.session_json()).unwrap();
    assert_eq!(advanced.last_turn_id, Some(turn_b));

    let error = store
        .read_items(thread)
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.text.as_deref() == Some("belongs to A"))
        .expect("dirty error is visible before restart");
    assert_eq!(error.turn_id, turn_a);
    assert_eq!(thread_usage_blocking(&paths).unwrap().total_tokens, 9);

    std::fs::write(paths.derived_dirty_json(), b"{}").unwrap();
    assert_eq!(
        store.read_items(thread).await.unwrap_err().code,
        ArkErrorCode::StoreFailure
    );
    assert_eq!(
        thread_usage_blocking(&paths).unwrap_err().code,
        ArkErrorCode::StoreFailure
    );
}

fn completed(thread: ark_common::ThreadId, turn: TurnId) -> RuntimeEvent {
    RuntimeEvent::TurnCompleted {
        seq: 0,
        thread_id: thread,
        turn_id: turn,
        timestamp: chrono::Utc::now(),
    }
}

fn usage(thread: ark_common::ThreadId, turn: TurnId, tokens: u64) -> RuntimeEvent {
    RuntimeEvent::Usage {
        seq: 0,
        thread_id: thread,
        turn_id: turn,
        timestamp: chrono::Utc::now(),
        usage: Usage::new(tokens, 0),
    }
}
