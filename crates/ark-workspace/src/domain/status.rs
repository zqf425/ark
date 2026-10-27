//! Status skeleton and git-output parsing (design 7.9).
//!
//! Every field of [`WorkspaceStatus`] is produced here from a raw git answer, so
//! the "unknown versus false" rule lives in exactly one place:
//!
//! * the command failed, or git is unavailable -> `None`;
//! * git answered -> `Some(value)`, including the negative case.
//!
//! The TypeScript inspector coerces a failed `status --porcelain` into
//! `isDirty: false`, which reports a clean tree for a workspace git refused to
//! read. The Rust contract keeps the distinction instead.

use ark_contract::WorkspaceStatus;
use chrono::Utc;

/// Status for a request that carried no path.
///
/// Mirrors `buildWorkspaceStatusResponse` in `kun/src/server/routes/workspace.ts`:
/// the response is still well formed, it just says nothing was asked about.
pub fn empty_status() -> WorkspaceStatus {
    WorkspaceStatus {
        path: String::new(),
        exists: false,
        is_git_repository: false,
        branch: None,
        head_sha: None,
        is_dirty: None,
        file_change_count: None,
        checked_at: Utc::now(),
    }
}

/// Status for a directory that exists but whose git facts are not yet known.
///
/// Used both for a non-repository directory (git facts stay `None`) and as the
/// base an inspector fills in for a repository.
pub fn base_status(path: &str, is_git_repository: bool) -> WorkspaceStatus {
    WorkspaceStatus {
        path: path.to_string(),
        exists: true,
        is_git_repository,
        branch: None,
        head_sha: None,
        is_dirty: None,
        file_change_count: None,
        checked_at: Utc::now(),
    }
}

/// Normalize `git rev-parse --abbrev-ref HEAD`.
///
/// A detached `HEAD` prints the literal `HEAD`, which is not a branch name and
/// must not be shown in the UI as one.
pub fn clean_branch(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "HEAD" {
        return None;
    }
    Some(trimmed.to_string())
}

/// Normalize `git rev-parse HEAD`.
pub fn clean_sha(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Count changed entries in `git status --porcelain` output.
///
/// One line is one changed path. Empty lines are ignored because the last record
/// may or may not be newline-terminated depending on the git build.
pub fn count_status_lines(raw: &str) -> u64 {
    raw.lines().filter(|line| !line.trim().is_empty()).count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_status_reports_nothing_known() {
        let status = empty_status();
        assert!(status.path.is_empty());
        assert!(!status.exists);
        assert!(!status.is_git_repository);
        assert!(status.branch.is_none());
        assert!(status.head_sha.is_none());
        assert!(status.is_dirty.is_none());
        assert!(status.file_change_count.is_none());
    }

    #[test]
    fn base_status_keeps_path_and_existence() {
        let status = base_status("/work/repo", false);
        assert_eq!(status.path, "/work/repo");
        assert!(status.exists);
        assert!(!status.is_git_repository);
        assert!(status.branch.is_none());
    }

    #[test]
    fn clean_branch_rejects_detached_head() {
        assert_eq!(clean_branch(" main\n"), Some("main".to_string()));
        assert_eq!(clean_branch("HEAD"), None);
        assert_eq!(clean_branch("   "), None);
    }

    #[test]
    fn clean_sha_trims_and_rejects_blank() {
        assert_eq!(clean_sha(" abc123 \n"), Some("abc123".to_string()));
        assert_eq!(clean_sha("\n"), None);
    }

    #[test]
    fn count_status_lines_ignores_blank_trailing_lines() {
        assert_eq!(count_status_lines(""), 0);
        assert_eq!(count_status_lines("?? a.txt\n"), 1);
        assert_eq!(count_status_lines(" M a.txt\n?? b.txt\n\n"), 2);
    }
}
