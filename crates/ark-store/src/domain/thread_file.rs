//! Thread metadata persisted as `thread.json` (design 7.4.1).
//!
//! The file also carries the non-portable fields (workspace, provider, model).
//! They are optional so an imported thread that cannot be resumed still loads.

use crate::domain::layout::ThreadPaths;
use ark_common::ThreadId;
use ark_contract::ThreadRuntimeSettings;
use ark_ports::ThreadMeta;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Parsed `thread.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadFile {
    pub thread_id: ThreadId,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub archived: bool,
    /// Set when this directory is a child run; parent id lives in the parent's
    /// `children/runs.jsonl`, this copy is only for local debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<ThreadId>,
    /// Absolute workspace path: portable content, non-portable semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<String>,
}

impl ThreadFile {
    pub fn new(thread_id: ThreadId, title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            thread_id,
            title: normalize_title(title),
            created_at: now,
            updated_at: now,
            archived: false,
            parent_thread_id: None,
            workspace: None,
            provider_id: None,
            model: None,
            autonomy: None,
        }
    }

    /// Mark the metadata as freshly changed without touching `created_at`.
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = normalize_title(title);
        self
    }

    pub fn to_meta(&self) -> ThreadMeta {
        ThreadMeta {
            thread_id: self.thread_id,
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            archived: self.archived,
            runtime: ThreadRuntimeSettings {
                workspace: self.workspace.clone(),
                provider_id: self.provider_id.clone(),
                model: self.model.clone(),
                autonomy: self.autonomy.clone(),
            },
        }
    }

    pub fn from_meta(meta: &ThreadMeta) -> Self {
        Self {
            thread_id: meta.thread_id,
            title: meta.title.clone(),
            created_at: meta.created_at,
            updated_at: meta.updated_at,
            archived: meta.archived,
            parent_thread_id: None,
            workspace: None,
            provider_id: None,
            model: None,
            autonomy: None,
        }
    }

    /// Re-point a fork's metadata at the new directory.
    pub fn forked_from(&self, thread_id: ThreadId) -> Self {
        let now = Utc::now();
        Self {
            thread_id,
            title: self.title.clone(),
            created_at: now,
            updated_at: now,
            archived: false,
            parent_thread_id: self.parent_thread_id,
            workspace: self.workspace.clone(),
            provider_id: self.provider_id.clone(),
            model: self.model.clone(),
            autonomy: self.autonomy.clone(),
        }
    }
}

pub fn normalize_title(title: impl Into<String>) -> String {
    let title = title.into();
    let trimmed = title.trim();
    if trimmed.is_empty() {
        "Untitled thread".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Read `thread.json`, or `None` when the directory is not a thread.
///
/// Goes through the shared encoding policy: a `thread.json` written by Notepad
/// carries a BOM, and a raw `serde_json::from_str` would then fail, making an
/// otherwise healthy session invisible.
pub fn read(paths: &ThreadPaths) -> Option<ThreadFile> {
    let raw = ark_common::read_text_opt(&paths.thread_json())?;
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_titles_are_normalized() {
        assert_eq!(normalize_title("   "), "Untitled thread");
        assert_eq!(normalize_title("  hello  "), "hello");
    }

    #[test]
    fn meta_roundtrip_keeps_identity() {
        let thread = ThreadId::new();
        let file = ThreadFile::new(thread, "hi").with_title("renamed");
        let meta = file.to_meta();
        assert_eq!(meta.thread_id, thread);
        assert_eq!(meta.title, "renamed");
        assert_eq!(ThreadFile::from_meta(&meta).title, "renamed");
    }

    #[test]
    fn fork_gets_a_new_identity_and_keeps_portable_fields() {
        let mut file = ThreadFile::new(ThreadId::new(), "parent");
        file.model = Some("deepseek-chat".into());
        let fork_id = ThreadId::new();
        let forked = file.forked_from(fork_id);
        assert_eq!(forked.thread_id, fork_id);
        assert_eq!(forked.model.as_deref(), Some("deepseek-chat"));
        assert_eq!(forked.title, "parent");
    }

    #[test]
    fn metadata_exposes_the_thread_runtime_overrides() {
        let mut file = ThreadFile::new(ThreadId::new(), "thread");
        file.workspace = Some("D:/code/project".into());
        file.provider_id = Some("fast".into());
        file.model = Some("model-v2".into());

        let runtime = file.to_meta().runtime;

        assert_eq!(runtime.workspace.as_deref(), Some("D:/code/project"));
        assert_eq!(runtime.provider_id.as_deref(), Some("fast"));
        assert_eq!(runtime.model.as_deref(), Some("model-v2"));
    }

    #[test]
    fn missing_file_reads_as_none() {
        let dir = tempfile::tempdir().expect("temp dir is creatable");
        let paths = ThreadPaths::new(dir.path().join("ark-store-missing-thread"));
        assert!(read(&paths).is_none());
    }
}
