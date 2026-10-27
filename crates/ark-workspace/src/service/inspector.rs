//! `WorkspaceInspector`: validate a candidate workspace path, then report status.
//!
//! Shape of the two public operations:
//!
//! * [`WorkspaceInspector::validate`] is the write-side gate: it rejects empty
//!   input, missing paths and non-directories, and returns the normalized
//!   absolute path a thread should be bound to. Its error messages are shown to
//!   the user as-is, so they name the offending path.
//! * [`WorkspaceInspector::status`] is the read-side probe used by
//!   `GET /v1/workspace/status`. It never fails: a missing directory, a non-git
//!   directory and an unusable git all degrade into contract fields.

use crate::domain::{
    base_status, clean_branch, clean_sha, count_status_lines, empty_status, resolve_candidate,
};
use crate::service::git_runner::run_git;
use ark_common::{ArkError, Result};
use ark_contract::WorkspaceStatus;
use std::path::{Path, PathBuf};

/// Stateless-ish inspector. `base` is the anchor for relative candidates and is
/// captured once so a status call cannot drift when the process changes its
/// working directory (the Electron host does chdir during startup).
#[derive(Debug, Clone)]
pub struct WorkspaceInspector {
    base: Option<PathBuf>,
}

impl Default for WorkspaceInspector {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceInspector {
    /// Inspector anchored at the process working directory.
    pub fn new() -> Self {
        Self {
            base: std::env::current_dir().ok(),
        }
    }

    /// Inspector anchored at an explicit directory. Used by tests and by hosts
    /// that already know their project root.
    pub fn with_base(base: impl Into<PathBuf>) -> Self {
        Self {
            base: Some(base.into()),
        }
    }

    /// The resolution anchor, if one is known.
    pub fn base(&self) -> Option<&Path> {
        self.base.as_deref()
    }

    /// Validate and normalize a candidate workspace path.
    ///
    /// `~` expands to the user profile, a relative path resolves against
    /// [`Self::base`], and surrounding whitespace is ignored. Symlinks are
    /// followed, so a symlinked directory passes. The returned path is *not*
    /// canonicalized: on Windows `canonicalize` yields a `\\?\` verbatim prefix
    /// that would be shown verbatim in the UI.
    pub fn validate(&self, raw: &str) -> Result<PathBuf> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ArkError::validation("workspace path must not be empty"));
        }
        let resolved = resolve_candidate(self.base.as_deref(), trimmed);
        match std::fs::metadata(&resolved) {
            Ok(meta) if meta.is_dir() => Ok(resolved),
            Ok(_) => Err(ArkError::validation(format!(
                "workspace path is not a directory: {}",
                resolved.display()
            ))),
            Err(_) => Err(ArkError::validation(format!(
                "workspace path does not exist: {}",
                resolved.display()
            ))),
        }
    }

    /// Report the workspace status for `path`.
    ///
    /// An empty (or whitespace-only) path returns the contract's empty status
    /// without touching the filesystem, matching the TypeScript route which
    /// answers a `?path=`-less request with `exists: false` and `null` fields.
    pub fn status(&self, path: &str) -> WorkspaceStatus {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return empty_status();
        }
        let resolved = resolve_candidate(self.base.as_deref(), trimmed);
        if !resolved.is_dir() {
            // A file, a dangling symlink and a nonexistent path all mean "there
            // is no workspace here"; no git call is attempted.
            let mut status = base_status(path, false);
            status.exists = false;
            return status;
        }
        self.repository_status(path, &resolved)
    }

    fn repository_status(&self, raw: &str, dir: &Path) -> WorkspaceStatus {
        if !is_work_tree(dir) {
            return base_status(raw, false);
        }
        let mut status = base_status(raw, true);
        status.branch =
            git_stdout(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).and_then(|s| clean_branch(&s));
        status.head_sha = git_stdout(dir, &["rev-parse", "HEAD"]).and_then(|s| clean_sha(&s));
        if let Some(output) = git_stdout(dir, &["status", "--porcelain"]) {
            let count = count_status_lines(&output);
            status.is_dirty = Some(count > 0);
            status.file_change_count = Some(count);
        }
        status
    }
}

/// `git rev-parse --is-inside-work-tree` prints exactly `true` inside a work
/// tree. Inside `.git`, or outside any repository, git fails and we report
/// "not a repository" rather than guessing.
fn is_work_tree(dir: &Path) -> bool {
    git_stdout(dir, &["rev-parse", "--is-inside-work-tree"])
        .map(|out| out.trim() == "true")
        .unwrap_or(false)
}

fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    run_git(dir, args).map(|output| output.stdout)
}

#[cfg(test)]
#[path = "inspector_tests.rs"]
mod tests;
