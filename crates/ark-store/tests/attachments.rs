use ark_common::ArkErrorCode;
use ark_store::{AttachmentStore, AttachmentUpload};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ark-attachment-{name}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&path).expect("temporary directory is creatable");
    path
}

#[test]
fn uploads_reads_and_deletes_binary_content() {
    let data_dir = temp_dir("roundtrip");
    let store = AttachmentStore::open(&data_dir, 1024).expect("store opens");
    let content = vec![0, 255, 16, 128];
    let entry = store
        .upload(AttachmentUpload {
            original_filename: "diagram.png".to_string(),
            mime_type: Some("image/png".to_string()),
            preview_text: Some("diagram preview".to_string()),
            content: content.clone(),
        })
        .expect("upload succeeds");

    assert_eq!(entry.original_filename, "diagram.png");
    assert_eq!(entry.byte_size, content.len() as u64);
    assert_eq!(store.get(entry.id).expect("metadata is readable"), entry);
    assert_eq!(store.read(entry.id).expect("content is readable"), content);
    assert_eq!(
        store
            .diagnostics()
            .expect("diagnostics are readable")
            .entry_count,
        1
    );

    store.delete(entry.id).expect("delete succeeds");
    assert_eq!(
        store.get(entry.id).unwrap_err().code,
        ArkErrorCode::NotFound
    );
    assert_eq!(
        store.read(entry.id).unwrap_err().code,
        ArkErrorCode::NotFound
    );
    let _ = std::fs::remove_dir_all(data_dir);
}

#[test]
fn rejects_content_larger_than_configured_limit_before_writing() {
    let data_dir = temp_dir("limit");
    let store = AttachmentStore::open(&data_dir, 3).expect("store opens");
    let error = store
        .upload(AttachmentUpload {
            original_filename: "too-large.bin".to_string(),
            mime_type: None,
            preview_text: None,
            content: vec![1, 2, 3, 4],
        })
        .expect_err("oversized uploads are rejected");

    assert_eq!(error.code, ArkErrorCode::ValidationError);
    assert!(!data_dir.join("attachments.json").exists());
    assert!(std::fs::read_dir(data_dir.join("attachments"))
        .expect("attachment directory exists")
        .next()
        .is_none());
    let _ = std::fs::remove_dir_all(data_dir);
}
