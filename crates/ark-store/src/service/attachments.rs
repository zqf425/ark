//! Atomic file-backed attachment storage.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ark_common::{read_json_file_opt, write_json_file, ArkError, AttachmentId, Result};
use ark_contract::{AttachmentDiagnostics, AttachmentEntry};
use ark_ports::{AttachmentStore as AttachmentStorePort, AttachmentUpload};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::store::fs_util::write_atomic;

/// Default maximum upload size: 25 MiB.
pub const DEFAULT_MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

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
    pub fn diagnostics(&self) -> Result<AttachmentDiagnostics> {
        let _guard = self.lock()?;
        let document = self.read_document();
        Ok(AttachmentDiagnostics {
            storage_path: self.attachments_dir.to_string_lossy().to_string(),
            entry_count: document.entries.len() as u64,
            total_bytes: document.entries.iter().map(|entry| entry.byte_size).sum(),
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
        Ok(())
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

    fn diagnostics(&self) -> Result<AttachmentDiagnostics> {
        Self::diagnostics(self)
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
            store.diagnostics().expect("diagnostics work").entry_count,
            0
        );
    }
}
