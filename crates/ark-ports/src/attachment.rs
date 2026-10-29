//! Attachment persistence boundary.

use ark_common::{AttachmentId, Result};
use ark_contract::{AttachmentDiagnostics, AttachmentEntry};

/// Raw attachment bytes and metadata supplied by the HTTP adapter.
#[derive(Debug, Clone)]
pub struct AttachmentUpload {
    pub original_filename: String,
    pub mime_type: Option<String>,
    pub preview_text: Option<String>,
    pub content: Vec<u8>,
}

/// Immutable attachment storage used by the HTTP adapter and runtime.
pub trait AttachmentStore: Send + Sync {
    /// Return the maximum accepted decoded upload size in bytes.
    fn max_bytes(&self) -> u64;

    /// Store an attachment and return the generated metadata.
    fn upload(&self, upload: AttachmentUpload) -> Result<AttachmentEntry>;

    /// Return metadata only when the attachment still exists.
    fn get(&self, id: AttachmentId) -> Result<AttachmentEntry>;

    /// Read the complete original byte stream for an existing attachment.
    fn read(&self, id: AttachmentId) -> Result<Vec<u8>>;

    /// Delete an attachment and its metadata.
    fn delete(&self, id: AttachmentId) -> Result<()>;

    /// Return the current persistent attachment-store usage.
    fn diagnostics(&self) -> Result<AttachmentDiagnostics>;
}
