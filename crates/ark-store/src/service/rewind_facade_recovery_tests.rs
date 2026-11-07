use crate::service::store::FileSessionStore;
use crate::store::tests::TempDir;
use ark_common::{ArkErrorCode, ThreadId, TurnId, Usage};
use ark_contract::RuntimeEvent;
use ark_ports::SessionStore;

struct RewindFixture {
    store: FileSessionStore,
    thread: ThreadId,
    retained_turn: TurnId,
    paths: crate::domain::layout::ThreadPaths,
    _dir: TempDir,
}

async fn failed_rewind_fixture(name: &str) -> RewindFixture {
    let dir = TempDir::new(name);
    let store = crate::service::store::tests::test_store(&dir);
    let thread = store
        .create_thread("rewind facade recovery")
        .await
        .unwrap()
        .thread_id;
    let paths = store.thread_paths(thread);
    let retained_turn = TurnId::new();
    let removed_turn = TurnId::new();

    for event in [
        error(thread, "retained item"),
        usage(thread, retained_turn, 3),
        error(thread, "removed item"),
        usage(thread, removed_turn, 7),
    ] {
        store.append_event(thread, event).await.unwrap();
    }
    crate::store::rewind_tx::fail_once(&paths, crate::store::rewind_tx::TestStage::DerivedRebuild);
    assert!(store
        .rewind_thread(thread, 3, Some(retained_turn))
        .await
        .is_err());
    assert!(paths.rewind_pending_json().exists());

    RewindFixture {
        store,
        thread,
        retained_turn,
        paths,
        _dir: dir,
    }
}

#[tokio::test]
async fn facade_read_items_recovers_failed_rewind_in_same_process() {
    let fixture = failed_rewind_fixture("rewind-facade-items").await;

    let items = fixture.store.read_items(fixture.thread).await.unwrap();

    assert!(items
        .iter()
        .any(|item| item.text.as_deref() == Some("retained item")));
    assert!(!items
        .iter()
        .any(|item| item.text.as_deref() == Some("removed item")));
    assert!(!fixture.paths.rewind_pending_json().exists());
}

#[tokio::test]
async fn production_usage_replay_recovers_failed_rewind_in_same_process() {
    let fixture = failed_rewind_fixture("rewind-facade-usage").await;

    let events = fixture
        .store
        .replay_events(fixture.thread, 0)
        .await
        .unwrap();
    let usage_events = events.iter().filter_map(|event| match event {
        RuntimeEvent::Usage { turn_id, usage, .. } => Some((*turn_id, usage.total_tokens)),
        _ => None,
    });

    assert_eq!(
        usage_events.collect::<Vec<_>>(),
        vec![(fixture.retained_turn, 3)]
    );
    assert!(!fixture.paths.rewind_pending_json().exists());
}

fn error(thread: ThreadId, message: &str) -> RuntimeEvent {
    RuntimeEvent::Error {
        seq: 0,
        thread_id: thread,
        timestamp: chrono::Utc::now(),
        code: ArkErrorCode::ModelRequestFailed,
        message: message.into(),
    }
}

fn usage(thread: ThreadId, turn: TurnId, total_tokens: u64) -> RuntimeEvent {
    RuntimeEvent::Usage {
        seq: 0,
        thread_id: thread,
        turn_id: turn,
        timestamp: chrono::Utc::now(),
        usage: Usage::new(total_tokens, 0),
    }
}
