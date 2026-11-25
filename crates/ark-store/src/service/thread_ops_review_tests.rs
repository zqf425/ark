use crate::service::store::tests::test_store;
use crate::store::tests::{event_at, TempDir};
use ark_common::{ArkErrorCode, ChildId, ThreadId, TurnId};
use ark_ports::{ChildRunRecord, SessionStore};
use std::sync::Arc;
use std::time::Duration;

fn child_record(parent: ThreadId, child: ChildId) -> ChildRunRecord {
    ChildRunRecord::queued(
        child,
        parent,
        TurnId::new(),
        "delete-race",
        chrono::Utc::now(),
    )
}

#[tokio::test]
async fn a_fork_of_a_compacted_source_is_rejected_instead_of_losing_derived_history() {
    let dir = TempDir::new("thread-ops-fork-compacted");
    let store = test_store(&dir);
    let source = store.create_thread("source").await.unwrap().thread_id;
    let paths = store.resolve_paths(source);
    let writer = store.registry.writer_for(source, &paths).unwrap();
    for _ in 0..4 {
        writer.append(event_at(source, 0)).await.unwrap();
    }
    assert_eq!(writer.compact(1).await.unwrap(), Some(3));

    let error = store.fork_thread(source, 4).await.unwrap_err();
    assert_eq!(error.code, ArkErrorCode::Conflict);
    assert!(error.message.contains("compacted"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_append_cannot_land_while_that_child_is_being_deleted() {
    let dir = TempDir::new("thread-ops-child-delete-race");
    let store = Arc::new(test_store(&dir));
    let parent = store.create_thread("parent").await.unwrap().thread_id;
    let child = ChildId::new();
    store
        .create_child_run(parent, child, "child")
        .await
        .unwrap();
    store
        .append_child_run(child_record(parent, child))
        .await
        .unwrap();

    store.pause_next_child_delete();
    let deleting = {
        let store = Arc::clone(&store);
        tokio::spawn(async move { store.delete_child_run(parent, child).await.unwrap() })
    };
    tokio::time::timeout(
        Duration::from_secs(2),
        store.child_delete_test_gate.entered.notified(),
    )
    .await
    .expect("deletion must reach the test gate");

    let appended = store.append_child_run(child_record(parent, child)).await;
    store.child_delete_test_gate.release.notify_one();
    assert!(deleting.await.unwrap());
    let error = appended.unwrap_err();
    assert_eq!(error.code, ArkErrorCode::Conflict);
    assert!(store.list_child_runs(parent).await.unwrap().is_empty());
    assert!(!store.paths.child(&parent, &child).dir().exists());
}

#[tokio::test]
async fn metadata_refresh_preserves_event_owned_index_fields() {
    let dir = TempDir::new("thread-ops-metadata-index-race");
    let store = test_store(&dir);
    let file = store
        .require_thread_file(store.create_thread_blocking("original").unwrap().thread_id)
        .unwrap();
    let thread = file.thread_id;
    let event_at = chrono::Utc::now() + Duration::from_secs(1);
    store.index_advance(thread, 9, Some(event_at));

    store.index_upsert(&file, 3).unwrap();

    let entry = store.lock_index().unwrap().get(&thread).cloned().unwrap();
    assert_eq!(entry.indexed_seq, 9);
    assert_eq!(entry.last_event_at, Some(event_at));
}
