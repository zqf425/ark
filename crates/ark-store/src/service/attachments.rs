//! Atomic file-backed attachment storage.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ark_common::{
    read_json_file_opt, write_json_file, ArkError, ArkErrorCode, AttachmentId, Result,
};
use ark_contract::{AttachmentDiagnostics, AttachmentEntry};
use ark_ports::{AttachmentStore as AttachmentStorePort, AttachmentUpload};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::fs_util::write_atomic;

/// Default maximum upload size: 25 MiB.
pub const DEFAULT_MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

/// Age at which an unassociated attachment counts as an orphan.
///
/// Diagnostics only: nothing collects these yet, so the number is a review
/// signal for a future retention policy, not a deletion rule.
pub const ORPHAN_AGE_DAYS: i64 = 30;

/// Bytes inspected by the declared-MIME consistency check.
///
/// 16 bytes cover every signature in [`magic_matches`] (WEBP needs 12).
const MAGIC_PREFIX_LEN: usize = 16;

/// Filesystem implementation of the attachment persistence port.
pub struct AttachmentStore {
    attachments_dir: PathBuf,
    metadata_path: PathBuf,
    max_bytes: u64,
    lock: Mutex<()>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AttachmentDocument {
    entries: Vec<AttachmentEntry>,
}

impl AttachmentStore {
    /// Open an attachment store rooted in `data_dir`.
    pub fn open(data_dir: impl AsRef<Path>, max_bytes: u64) -> Result<Self> {
        if max_bytes == 0 {
            return Err(ArkError::validation(
                "attachment size limit must be positive",
            ));
        }
        let data_dir = data_dir.as_ref().to_path_buf();
        let attachments_dir = data_dir.join("attachments");
        std::fs::create_dir_all(&attachments_dir)?;
        Ok(Self {
            metadata_path: data_dir.join("attachments.json"),
            attachments_dir,
            max_bytes,
            lock: Mutex::new(()),
        })
    }

    /// Store one immutable attachment and return its generated metadata.
    pub fn upload(&self, upload: AttachmentUpload) -> Result<AttachmentEntry> {
        let _guard = self.lock()?;
        self.validate_upload(&upload)?;
        let id = AttachmentId::new();
        let entry = AttachmentEntry {
            id,
            original_filename: upload.original_filename,
            byte_size: upload.content.len() as u64,
            mime_type: upload.mime_type,
            created_at: Utc::now(),
            preview_text: upload.preview_text,
            sha256: sha256_hex(&upload.content),
        };
        write_atomic(&self.content_path(id), &upload.content)?;
        let mut document = self.read_document();
        document.entries.push(entry.clone());
        if let Err(error) = self.write_document(&document) {
            let _ = std::fs::remove_file(self.content_path(id));
            return Err(error);
        }
        Ok(entry)
    }

    /// Return the maximum accepted decoded upload size in bytes.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Look up metadata only for a fully persisted attachment.
    pub fn get(&self, id: AttachmentId) -> Result<AttachmentEntry> {
        let _guard = self.lock()?;
        let entry = self.find(id)?;
        if self.content_path(id).is_file() {
            Ok(entry)
        } else {
            Err(ArkError::not_found(format!(
                "attachment {id} does not exist"
            )))
        }
    }

    /// Read the original bytes for a fully persisted attachment.
    pub fn read(&self, id: AttachmentId) -> Result<Vec<u8>> {
        self.get(id)?;
        std::fs::read(self.content_path(id)).map_err(|error| {
            ArkError::not_found(format!("attachment {id} does not exist: {error}"))
        })
    }

    /// Delete metadata first, making an interrupted deletion invisible to readers.
    pub fn delete(&self, id: AttachmentId) -> Result<()> {
        let _guard = self.lock()?;
        let mut document = self.read_document();
        let before = document.entries.len();
        document.entries.retain(|entry| entry.id != id);
        if before == document.entries.len() {
            return Err(ArkError::not_found(format!(
                "attachment {id} does not exist"
            )));
        }
        self.write_document(&document)?;
        match std::fs::remove_file(self.content_path(id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Summarize the persistent attachment store.
    ///
    /// `associated` comes from the caller because association lives in the
    /// thread event log: this store owns bytes and never reads threads, so it
    /// can only report orphans against a set someone else projected.
    pub fn diagnostics(&self, associated: &HashSet<AttachmentId>) -> Result<AttachmentDiagnostics> {
        let _guard = self.lock()?;
        let document = self.read_document();
        let orphan_cutoff = Utc::now() - Duration::days(ORPHAN_AGE_DAYS);
        Ok(AttachmentDiagnostics {
            storage_path: self.attachments_dir.to_string_lossy().to_string(),
            entry_count: document.entries.len() as u64,
            total_bytes: document.entries.iter().map(|entry| entry.byte_size).sum(),
            orphan_count: document
                .entries
                .iter()
                .filter(|entry| !associated.contains(&entry.id) && entry.created_at < orphan_cutoff)
                .count() as u64,
        })
    }

    fn validate_upload(&self, upload: &AttachmentUpload) -> Result<()> {
        if upload.original_filename.trim().is_empty() {
            return Err(ArkError::validation(
                "attachment filename must not be empty",
            ));
        }
        if upload.content.len() as u64 > self.max_bytes {
            return Err(ArkError::validation(format!(
                "attachment exceeds the {max} byte limit",
                max = self.max_bytes
            )));
        }
        validate_declared_mime(upload)
    }

    fn find(&self, id: AttachmentId) -> Result<AttachmentEntry> {
        self.read_document()
            .entries
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| ArkError::not_found(format!("attachment {id} does not exist")))
    }

    fn content_path(&self, id: AttachmentId) -> PathBuf {
        self.attachments_dir.join(id.to_string())
    }

    fn read_document(&self) -> AttachmentDocument {
        read_json_file_opt(&self.metadata_path).unwrap_or_default()
    }

    fn write_document(&self, document: &AttachmentDocument) -> Result<()> {
        write_json_file(&self.metadata_path, document)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.lock
            .lock()
            .map_err(|_| ArkError::internal("attachment store lock poisoned"))
    }
}

/// Lowercase hex SHA-256 of the stored bytes.
fn sha256_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Reject a declared MIME that its own leading bytes contradict.
///
/// Only five formats are checked, because those are the ones whose signatures
/// are unambiguous in 16 bytes. An undeclared MIME or an unrecognized one is
/// accepted deliberately: guessing at an unknown byte layout would reject valid
/// uploads, and the download response already carries `nosniff`, so a wrong
/// declaration cannot become executable content.
fn validate_declared_mime(upload: &AttachmentUpload) -> Result<()> {
    let Some(declared) = upload.mime_type.as_deref() else {
        return Ok(());
    };
    let normalized = normalize_mime(declared);
    let prefix = &upload.content[..upload.content.len().min(MAGIC_PREFIX_LEN)];
    let Some(matches) = magic_matches(&normalized, prefix) else {
        return Ok(());
    };
    if matches {
        Ok(())
    } else {
        Err(ArkError::new(
            ArkErrorCode::MimeMismatch,
            format!("attachment bytes are not consistent with the declared {normalized} type"),
        ))
    }
}

/// `Image/PNG; charset=binary` -> `image/png`.
fn normalize_mime(declared: &str) -> String {
    declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// `Some(true/false)` for checked formats, `None` when the type is not checked.
fn magic_matches(mime: &str, prefix: &[u8]) -> Option<bool> {
    Some(match mime {
        "image/png" => prefix.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" | "image/jpg" => prefix.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a"),
        "image/webp" => {
            prefix.starts_with(b"RIFF") && prefix.get(8..12) == Some(b"WEBP".as_slice())
        }
        "application/pdf" => prefix.starts_with(b"%PDF-"),
        _ => return None,
    })
}

impl AttachmentStorePort for AttachmentStore {
    fn max_bytes(&self) -> u64 {
        Self::max_bytes(self)
    }

    fn upload(&self, upload: AttachmentUpload) -> Result<AttachmentEntry> {
        Self::upload(self, upload)
    }

    fn get(&self, id: AttachmentId) -> Result<AttachmentEntry> {
        Self::get(self, id)
    }

    fn read(&self, id: AttachmentId) -> Result<Vec<u8>> {
        Self::read(self, id)
    }

    fn delete(&self, id: AttachmentId) -> Result<()> {
        Self::delete(self, id)
    }

    fn diagnostics(&self, associated: &HashSet<AttachmentId>) -> Result<AttachmentDiagnostics> {
        Self::diagnostics(self, associated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_temp_file_without_metadata_after_interrupted_upload() {
        let dir = tempfile::tempdir().expect("temp dir is creatable");
        let store = AttachmentStore::open(dir.path(), 16).expect("store opens");
        std::fs::write(store.attachments_dir.join(".partial.tmp"), b"partial")
            .expect("temp writes");
        assert_eq!(
            store
                .diagnostics(&HashSet::new())
                .expect("diagnostics work")
                .entry_count,
            0
        );
    }

    #[test]
    fn rejects_bytes_that_contradict_the_declared_mime() {
        let dir = tempfile::tempdir().expect("temp dir is creatable");
        let store = AttachmentStore::open(dir.path(), 1024).expect("store opens");
        let error = store
            .upload(AttachmentUpload {
                original_filename: "not-really.png".to_string(),
                mime_type: Some("image/png".to_string()),
                preview_text: None,
                content: b"plain text, no signature".to_vec(),
            })
            .expect_err("mismatched mime is rejected");
        assert_eq!(error.code, ArkErrorCode::MimeMismatch);
    }

    #[test]
    fn accepts_unrecognized_types_without_a_magic_check() {
        let dir = tempfile::tempdir().expect("temp dir is creatable");
        let store = AttachmentStore::open(dir.path(), 1024).expect("store opens");
        let entry = store
            .upload(AttachmentUpload {
                original_filename: "notes.txt".to_string(),
                mime_type: Some("text/plain".to_string()),
                preview_text: None,
                content: b"hello".to_vec(),
            })
            .expect("unchecked types upload");
        assert_eq!(entry.mime_type.as_deref(), Some("text/plain"));
    }

    #[test]
    fn records_a_lowercase_hex_sha256_of_the_stored_bytes() {
        let dir = tempfile::tempdir().expect("temp dir is creatable");
        let store = AttachmentStore::open(dir.path(), 1024).expect("store opens");
        // SHA-256 of the empty input, a stable known-answer value.
        let entry = store
            .upload(AttachmentUpload {
                original_filename: "empty.bin".to_string(),
                mime_type: None,
                preview_text: None,
                content: Vec::new(),
            })
            .expect("upload succeeds");
        assert_eq!(
            entry.sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
