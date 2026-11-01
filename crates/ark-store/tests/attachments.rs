use ark_common::{ArkErrorCode, AttachmentId as ArkAttachmentId};
use ark_store::{AttachmentStore, AttachmentUpload};

fn temp_dir(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("ark-attachment-{name}-"))
        .tempdir()
        .expect("temporary directory is creatable")
}

#[test]
fn uploads_reads_and_deletes_binary_content() {
    let dir = temp_dir("roundtrip");
    let data_dir = dir.path();
    let store = AttachmentStore::open(data_dir, 1024).expect("store opens");
    let content = vec![0, 255, 16, 128];
    let entry = store
        .upload(AttachmentUpload {
            original_filename: "diagram.png".to_string(),
            // Deliberately an unchecked type: this case is about arbitrary bytes
            // round-tripping, and arbitrary bytes must not be declared as PNG --
            // that is exactly what the magic-byte check now rejects.
            mime_type: Some("application/octet-stream".to_string()),
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
            .diagnostics(&Default::default())
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
}

#[test]
fn rejects_content_larger_than_configured_limit_before_writing() {
    let dir = temp_dir("limit");
    let data_dir = dir.path();
    let store = AttachmentStore::open(data_dir, 3).expect("store opens");
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
}

/// Rewrite one entry's `created_at` so the orphan age rule can be exercised
/// without waiting 30 days.
fn backdate(data_dir: &std::path::Path, id: ArkAttachmentId, days: i64) {
    let path = data_dir.join("attachments.json");
    let raw = std::fs::read_to_string(&path).expect("metadata reads");
    let mut document: serde_json::Value = serde_json::from_str(&raw).expect("metadata is json");
    let entries = document["entries"]
        .as_array_mut()
        .expect("entries is an array");
    for entry in entries.iter_mut() {
        if entry["id"].as_str() == Some(&id.to_string()) {
            let created = entry["created_at"]
                .as_str()
                .expect("created_at is a string");
            let parsed = chrono::DateTime::parse_from_rfc3339(created).expect("timestamp parses");
            entry["created_at"] = serde_json::Value::String(
                (parsed.with_timezone(&chrono::Utc) - chrono::Duration::days(days)).to_rfc3339(),
            );
        }
    }
    std::fs::write(
        &path,
        serde_json::to_string(&document).expect("metadata serializes"),
    )
    .expect("metadata writes");
}

#[test]
fn orphan_count_reports_only_old_unassociated_attachments() {
    let dir = temp_dir("orphans");
    let data_dir = dir.path();
    let store = AttachmentStore::open(data_dir, 1024).expect("store opens");
    let upload = |name: &str| {
        store
            .upload(AttachmentUpload {
                original_filename: name.to_string(),
                mime_type: None,
                preview_text: None,
                content: name.as_bytes().to_vec(),
            })
            .expect("upload succeeds")
    };
    let fresh = upload("fresh.bin");
    let old_orphan = upload("old-orphan.bin");
    let old_associated = upload("old-associated.bin");
    backdate(data_dir, old_orphan.id, 31);
    backdate(data_dir, old_associated.id, 31);

    let associated = [old_associated.id].into_iter().collect();
    let diagnostics = store
        .diagnostics(&associated)
        .expect("diagnostics are readable");

    assert_eq!(diagnostics.entry_count, 3);
    assert_eq!(
        diagnostics.orphan_count, 1,
        "only the old, unassociated entry is an orphan"
    );
    // The fresh entry proves the age rule, not just the association rule.
    assert!(!associated.contains(&fresh.id));
}
