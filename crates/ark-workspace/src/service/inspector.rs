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
use ark_contract::{WorkspaceDiff, WorkspaceDiffFile, WorkspaceStatus};
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

    /// Read a bounded, side-effect-free Git diff for an already validated root.
    ///
    /// Untracked contents are intentionally never read: merely listing an
    /// untracked secret must not send it to the renderer. Git owns all parsing
    /// and command execution in this crate.
    pub fn diff(&self, root: &Path) -> Result<WorkspaceDiff> {
        if !root.is_dir() {
            return Err(ArkError::validation(format!(
                "workspace path is not a directory: {}",
                root.display()
            )));
        }
        let canonical = root.canonicalize().map_err(|_| {
            ArkError::validation(format!("cannot canonicalize workspace: {}", root.display()))
        })?;
        if !is_work_tree(&canonical) {
            return Err(ArkError::validation("workspace is not a git repository"));
        }
        let raw = git_stdout(&canonical, &["status", "--porcelain=v1", "-z"])
            .ok_or_else(|| ArkError::internal("git status failed"))?;
        let mut diff = WorkspaceDiff {
            head_sha: git_stdout(&canonical, &["rev-parse", "HEAD"])
                .and_then(|value| clean_sha(&value)),
            ..WorkspaceDiff::default()
        };
        for entry in parse_porcelain(&raw) {
            match entry.group {
                DiffGroup::Staged => diff.staged.push(entry.file),
                DiffGroup::Unstaged => diff.unstaged.push(entry.file),
                DiffGroup::Untracked => diff.untracked.push(entry.file),
            }
        }
        diff.file_count = (diff.staged.len() + diff.unstaged.len() + diff.untracked.len()) as u64;
        let mut staged = std::mem::take(&mut diff.staged);
        attach_patch(
            &canonical,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
            ],
            &mut staged,
            &mut diff,
        );
        diff.staged = staged;
        let mut unstaged = std::mem::take(&mut diff.unstaged);
        attach_patch(
            &canonical,
            &["diff", "--no-ext-diff", "--no-textconv", "--no-color"],
            &mut unstaged,
            &mut diff,
        );
        diff.unstaged = unstaged;
        Ok(diff)
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

const MAX_DIFF_FILES: usize = 200;
const MAX_PATCH_BYTES: usize = 256 * 1024;

enum DiffGroup {
    Staged,
    Unstaged,
    Untracked,
}

struct ParsedDiffFile {
    group: DiffGroup,
    file: WorkspaceDiffFile,
}

fn parse_porcelain(raw: &str) -> Vec<ParsedDiffFile> {
    let mut parts = raw.split('\0').filter(|part| !part.is_empty());
    let mut result = Vec::new();
    while let Some(record) = parts.next() {
        if result.len() >= MAX_DIFF_FILES {
            break;
        }
        if record.len() < 4 {
            continue;
        }
        let code = &record[..2];
        let path = record[3..].to_string();
        if code == "??" {
            result.push(ParsedDiffFile {
                group: DiffGroup::Untracked,
                file: diff_file(path, code, None),
            });
            continue;
        }
        let renamed =
            matches!(code.as_bytes()[0], b'R' | b'C') || matches!(code.as_bytes()[1], b'R' | b'C');
        let previous_path = renamed.then(|| parts.next().unwrap_or_default().to_string());
        if code.as_bytes()[0] != b' ' {
            result.push(ParsedDiffFile {
                group: DiffGroup::Staged,
                file: diff_file(path.clone(), code, previous_path.clone()),
            });
        }
        if code.as_bytes()[1] != b' ' {
            result.push(ParsedDiffFile {
                group: DiffGroup::Unstaged,
                file: diff_file(path, code, previous_path),
            });
        }
    }
    result
}

fn diff_file(path: String, status: &str, previous_path: Option<String>) -> WorkspaceDiffFile {
    WorkspaceDiffFile {
        path,
        status: status.to_string(),
        previous_path,
        binary: false,
        patch: None,
        truncated: false,
    }
}

fn attach_patch(
    root: &Path,
    args: &[&str],
    files: &mut [WorkspaceDiffFile],
    diff: &mut WorkspaceDiff,
) {
    if files.is_empty() || diff.truncated {
        return;
    }
    let Some(output) = git_stdout(root, args) else {
        return;
    };
    let available = MAX_PATCH_BYTES.saturating_sub(diff.patch_bytes as usize);
    if available == 0 {
        diff.truncated = true;
        return;
    }
    let bytes = output.as_bytes();
    let (patch, truncated) = if bytes.len() > available {
        (
            String::from_utf8_lossy(&bytes[..available]).into_owned(),
            true,
        )
    } else {
        (output, false)
    };
    diff.patch_bytes += patch.len() as u64;
    diff.truncated |= truncated;
    let binary = patch.contains("Binary files ") || patch.contains("GIT binary patch");
    for file in files {
        file.binary = binary;
        file.patch = (!binary).then(|| patch.clone());
        file.truncated = truncated;
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
