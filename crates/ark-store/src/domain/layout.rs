//! Directory layout: a thread is a self-contained, movable directory.
//!
//! Design 7.4.1. Nothing outside this module may build a store path by joining
//! strings, so a layout change is one edit and every reader follows.

use ark_common::{ChildId, ThreadId};
use std::path::{Path, PathBuf};

/// Names of the files and directories inside one thread directory.
pub mod names {
    pub const THREAD_JSON: &str = "thread.json";
    pub const SESSION_JSON: &str = "session.json";
    pub const EVENTS_JSONL: &str = "events.jsonl";
    pub const COMPACTED_JSONL: &str = "events.jsonl.compacted";
    pub const ITEMS_JSONL: &str = "items.jsonl";
    pub const USAGE_JSONL: &str = "usage.jsonl";
    pub const ATTACHMENTS: &str = "attachments";
    pub const RUNS: &str = "runs";
    pub const CHILDREN: &str = "children";
    pub const CHILDREN_INDEX: &str = "index.json";
    pub const CHILDREN_RUNS: &str = "runs.jsonl";
    pub const THREADS: &str = "threads";
    pub const INDEX_JSON: &str = "index.json";
    pub const INDEX_DIRTY: &str = "index-dirty.json";
    pub const LOCK_FILE: &str = ".lock";
    /// Prefix of a children directory that is being deleted (design 7.4.1).
    pub const DELETING_PREFIX: &str = "children.deleting-";
}

/// Root of one data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPaths {
    root: PathBuf,
}

impl DataPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn threads_dir(&self) -> PathBuf {
        self.root.join(names::THREADS)
    }

    pub fn index_file(&self) -> PathBuf {
        self.root.join(names::INDEX_JSON)
    }

    pub fn index_dirty_file(&self) -> PathBuf {
        self.root.join(names::INDEX_DIRTY)
    }

    pub fn lock_file(&self) -> PathBuf {
        self.root.join(names::LOCK_FILE)
    }

    pub fn thread(&self, thread: &ThreadId) -> ThreadPaths {
        ThreadPaths::new(self.threads_dir().join(thread.to_string()))
    }

    pub fn child(&self, parent: &ThreadId, child: &ChildId) -> ThreadPaths {
        self.thread(parent)
            .children_dir()
            .join(child.to_string())
            .into()
    }

    /// Library-style relative path (`threads/<id>`) stored in the index.
    pub fn thread_rel(&self, thread: &ThreadId) -> String {
        format!("{}/{}", names::THREADS, thread)
    }

    /// Relative path for a child run (`threads/<parent>/children/<child>`).
    pub fn child_rel(&self, parent: &ThreadId, child: &ChildId) -> String {
        format!(
            "{}/{}/{}/{}",
            names::THREADS,
            parent,
            names::CHILDREN,
            child
        )
    }
}

/// Paths inside a single thread directory (parent thread or child run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadPaths {
    dir: PathBuf,
}

impl ThreadPaths {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn thread_json(&self) -> PathBuf {
        self.dir.join(names::THREAD_JSON)
    }

    pub fn session_json(&self) -> PathBuf {
        self.dir.join(names::SESSION_JSON)
    }

    pub fn events_jsonl(&self) -> PathBuf {
        self.dir.join(names::EVENTS_JSONL)
    }

    pub fn compacted_jsonl(&self) -> PathBuf {
        self.dir.join(names::COMPACTED_JSONL)
    }

    pub fn items_jsonl(&self) -> PathBuf {
        self.dir.join(names::ITEMS_JSONL)
    }

    pub fn usage_jsonl(&self) -> PathBuf {
        self.dir.join(names::USAGE_JSONL)
    }

    pub fn attachments_dir(&self) -> PathBuf {
        self.dir.join(names::ATTACHMENTS)
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.dir.join(names::RUNS)
    }

    pub fn children_dir(&self) -> PathBuf {
        self.dir.join(names::CHILDREN)
    }

    pub fn children_index(&self) -> PathBuf {
        self.children_dir().join(names::CHILDREN_INDEX)
    }

    pub fn children_runs(&self) -> PathBuf {
        self.children_dir().join(names::CHILDREN_RUNS)
    }

    /// Stable in-memory key for this directory (one writer per real directory).
    pub fn key(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }

    /// Whether `thread.json` exists, i.e. the directory is a real thread.
    pub fn exists(&self) -> bool {
        self.thread_json().is_file()
    }
}

impl From<PathBuf> for ThreadPaths {
    fn from(dir: PathBuf) -> Self {
        Self::new(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_dir_nests_under_the_parent() {
        let parent = ThreadId::new();
        let child = ChildId::new();
        let paths = DataPaths::new("/data");
        let child_paths = paths.child(&parent, &child);
        assert!(child_paths
            .dir()
            .ends_with(format!("{parent}/children/{child}")));
        assert_eq!(
            paths.child_rel(&parent, &child),
            format!("threads/{parent}/children/{child}")
        );
    }

    #[test]
    fn every_documented_file_has_one_accessor() {
        let paths = ThreadPaths::new("/data/threads/t1");
        assert!(paths.thread_json().ends_with("thread.json"));
        assert!(paths.session_json().ends_with("session.json"));
        assert!(paths.events_jsonl().ends_with("events.jsonl"));
        assert!(paths.compacted_jsonl().ends_with("events.jsonl.compacted"));
        assert!(paths.items_jsonl().ends_with("items.jsonl"));
        assert!(paths.usage_jsonl().ends_with("usage.jsonl"));
        assert!(paths.attachments_dir().ends_with("attachments"));
        assert!(paths.children_index().ends_with("children/index.json"));
        assert!(paths.children_runs().ends_with("children/runs.jsonl"));
    }

    #[test]
    fn writer_key_is_the_directory() {
        let paths = ThreadPaths::new("/data/threads/t1");
        assert_eq!(paths.key(), paths.dir().to_string_lossy());
    }
}
