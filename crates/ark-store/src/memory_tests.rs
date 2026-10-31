use std::sync::Arc;

use ark_contract::{CreateMemoryRequest, UpdateMemoryRequest};
use ark_ports::MemoryService;

use crate::MemoryFileStore;

fn temp_data_dir(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("ark-memory-{label}-"))
        .tempdir()
        .expect("data directory created")
}

#[test]
fn creates_updates_deletes_and_lists_memory_entries() {
    let dir = temp_data_dir("crud");
    let data_dir = dir.path();
    let store = MemoryFileStore::open(data_dir).expect("store opens");

    let created = store
        .create(CreateMemoryRequest {
            text: "用户偏好中文回答".to_string(),
            tags: vec!["preference".to_string()],
            source: Some("manual".to_string()),
        })
        .expect("create succeeds");
    let updated = store
        .update(
            &created.id,
            UpdateMemoryRequest {
                text: Some("用户偏好使用简体中文回答".to_string()),
                tags: Some(vec!["language".to_string()]),
                source: None,
            },
        )
        .expect("update succeeds");

    assert_eq!(updated.text, "用户偏好使用简体中文回答");
    assert_eq!(store.list().expect("list succeeds").entries.len(), 1);
    store.delete(&created.id).expect("delete succeeds");
    assert!(store.list().expect("list succeeds").entries.is_empty());
}

#[test]
fn empty_file_starts_empty_without_blocking_future_writes() {
    let dir = temp_data_dir("empty");
    let data_dir = dir.path();
    std::fs::write(data_dir.join("memory.json"), b"").expect("empty file written");

    let store = MemoryFileStore::open(data_dir).expect("empty file is recoverable");
    assert!(store.list().expect("list succeeds").entries.is_empty());
    store
        .create(CreateMemoryRequest {
            text: "可以继续写入".to_string(),
            tags: vec![],
            source: None,
        })
        .expect("write after recovery succeeds");
    assert_eq!(store.list().expect("list succeeds").entries.len(), 1);
}

#[test]
fn damaged_file_starts_empty_without_blocking_future_writes() {
    let dir = temp_data_dir("recovery");
    let data_dir = dir.path();
    std::fs::write(data_dir.join("memory.json"), b"not json").expect("damaged file written");

    let store = MemoryFileStore::open(data_dir).expect("damaged file is recoverable");
    assert!(store.list().expect("list succeeds").entries.is_empty());
    store
        .create(CreateMemoryRequest {
            text: "可以继续写入".to_string(),
            tags: vec![],
            source: None,
        })
        .expect("write after recovery succeeds");
    assert_eq!(store.list().expect("list succeeds").entries.len(), 1);
}

#[test]
fn concurrent_writes_keep_every_entry() {
    let dir = temp_data_dir("concurrent");
    let store = Arc::new(MemoryFileStore::open(dir.path()).expect("store opens"));
    let mut writers = Vec::new();
    for index in 0..16 {
        let store = Arc::clone(&store);
        writers.push(std::thread::spawn(move || {
            store
                .create(CreateMemoryRequest {
                    text: format!("entry {index}"),
                    tags: vec![],
                    source: None,
                })
                .expect("concurrent create succeeds");
        }));
    }
    for writer in writers {
        writer.join().expect("writer joins");
    }

    assert_eq!(store.list().expect("list succeeds").entries.len(), 16);
}

#[test]
fn rejects_an_oversized_text_without_changing_existing_entries() {
    let dir = temp_data_dir("limit");
    let store = MemoryFileStore::open(dir.path()).expect("store opens");
    store
        .create(CreateMemoryRequest {
            text: "保留的条目".to_string(),
            tags: vec![],
            source: None,
        })
        .expect("initial create succeeds");

    let error = store
        .create(CreateMemoryRequest {
            text: "x".repeat(16 * 1024 + 1),
            tags: vec![],
            source: None,
        })
        .expect_err("oversized text is rejected");
    assert!(error.message.contains("16384"));
    assert_eq!(store.list().expect("list succeeds").entries.len(), 1);
}

#[test]
fn read_entries_observes_disk_state_after_the_store_was_opened() {
    let data_dir = temp_data_dir("read-entries");
    std::fs::create_dir_all(&data_dir).expect("data directory created");

    // Absent document is an empty store, not a failure: injection must stay quiet.
    let store = MemoryFileStore::open(&data_dir).expect("store opens");
    assert!(store
        .read_entries()
        .expect("absent file reads empty")
        .is_empty());

    // A second store writing after this one opened proves the read is from disk
    // rather than from the snapshot captured by `open`.
    let writer = MemoryFileStore::open(&data_dir).expect("writer opens");
    writer
        .create(CreateMemoryRequest {
            text: "下一次回合可见".to_string(),
            tags: vec![],
            source: None,
        })
        .expect("create succeeds");
    let entries = store.read_entries().expect("fresh read succeeds");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].text, "下一次回合可见");

    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn read_entries_reports_a_damaged_document_instead_of_silently_emptying() {
    let data_dir = temp_data_dir("read-damaged");
    std::fs::create_dir_all(&data_dir).expect("data directory created");
    std::fs::write(data_dir.join("memory.json"), b"not json").expect("damaged file written");

    // Injection needs to warn about a damaged document, so this must be an
    // error even though `open` deliberately recovers with an empty snapshot.
    let store = MemoryFileStore::open(&data_dir).expect("damaged file is recoverable");
    assert!(store.read_entries().is_err());

    let _ = std::fs::remove_dir_all(data_dir);
}
