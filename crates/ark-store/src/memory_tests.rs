use std::sync::Arc;

use ark_contract::{CreateMemoryRequest, UpdateMemoryRequest};
use ark_ports::MemoryService;

use crate::MemoryFileStore;

fn temp_data_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ark-memory-{label}-{}", uuid::Uuid::new_v4()))
}

#[test]
fn creates_updates_deletes_and_lists_memory_entries() {
    let data_dir = temp_data_dir("crud");
    let store = MemoryFileStore::open(&data_dir).expect("store opens");

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

    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn empty_file_starts_empty_without_blocking_future_writes() {
    let data_dir = temp_data_dir("empty");
    std::fs::create_dir_all(&data_dir).expect("data directory created");
    std::fs::write(data_dir.join("memory.json"), b"").expect("empty file written");

    let store = MemoryFileStore::open(&data_dir).expect("empty file is recoverable");
    assert!(store.list().expect("list succeeds").entries.is_empty());
    store
        .create(CreateMemoryRequest {
            text: "可以继续写入".to_string(),
            tags: vec![],
            source: None,
        })
        .expect("write after recovery succeeds");
    assert_eq!(store.list().expect("list succeeds").entries.len(), 1);

    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn damaged_file_starts_empty_without_blocking_future_writes() {
    let data_dir = temp_data_dir("recovery");
    std::fs::create_dir_all(&data_dir).expect("data directory created");
    std::fs::write(data_dir.join("memory.json"), b"not json").expect("damaged file written");

    let store = MemoryFileStore::open(&data_dir).expect("damaged file is recoverable");
    assert!(store.list().expect("list succeeds").entries.is_empty());
    store
        .create(CreateMemoryRequest {
            text: "可以继续写入".to_string(),
            tags: vec![],
            source: None,
        })
        .expect("write after recovery succeeds");
    assert_eq!(store.list().expect("list succeeds").entries.len(), 1);

    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn concurrent_writes_keep_every_entry() {
    let data_dir = temp_data_dir("concurrent");
    let store = Arc::new(MemoryFileStore::open(&data_dir).expect("store opens"));
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
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn rejects_an_oversized_text_without_changing_existing_entries() {
    let data_dir = temp_data_dir("limit");
    let store = MemoryFileStore::open(&data_dir).expect("store opens");
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

    let _ = std::fs::remove_dir_all(data_dir);
}
