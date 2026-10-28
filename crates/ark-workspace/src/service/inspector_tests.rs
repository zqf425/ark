//! Behaviour tests for [`WorkspaceInspector`].
//!
//! Ground rules kept by every case here:
//!
//! * all fixtures live under `std::env::temp_dir()`, never in the repository,
//!   the user profile or any network share;
//! * a git-dependent case skips itself when `git` is not on `PATH` — on such a
//!   machine these cases must be marked `#[ignore]` instead, because a silent
//!   skip would hide the regression they exist to catch.
//!
//! A tiny `TempDir` helper replaces the `tempfile` crate so the crate keeps its
//! dependency set unchanged (no registry access needed to build it).

use super::*;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Removes its directory on drop, including on assertion failure.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "ark-ws-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Run one git command inside `dir` and assert it succeeded.
fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Ark Test",
            "-c",
            "user.email=ark@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git should be runnable");
    assert!(status.success(), "git {:?} failed", args);
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "--quiet"]);
    std::fs::write(dir.join("README.md"), "hello\n").expect("write tracked file");
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "--quiet", "-m", "init"]);
}

#[test]
fn empty_path_returns_empty_status() {
    let inspector = WorkspaceInspector::new();
    let status = inspector.status("");
    assert!(status.path.is_empty());
    assert!(!status.exists);
    assert!(!status.is_git_repository);
    assert!(status.branch.is_none());
    assert!(status.head_sha.is_none());
    assert!(status.is_dirty.is_none());
    assert!(status.file_change_count.is_none());
}

#[test]
fn missing_directory_is_not_a_workspace() {
    let fixture = TempDir::new("missing");
    let candidate = fixture.path().join("does-not-exist");
    let inspector = WorkspaceInspector::new();
    let status = inspector.status(&candidate.to_string_lossy());
    assert_eq!(status.path, candidate.to_string_lossy());
    assert!(!status.exists);
    assert!(!status.is_git_repository);
    assert!(status.is_dirty.is_none());
    assert!(status.file_change_count.is_none());
}

#[test]
fn plain_directory_is_not_a_repository() {
    let fixture = TempDir::new("plain");
    let inspector = WorkspaceInspector::new();
    let status = inspector.status(&fixture.path().to_string_lossy());
    assert!(status.exists);
    assert!(!status.is_git_repository);
    assert!(status.branch.is_none());
    assert!(status.head_sha.is_none());
    assert!(status.is_dirty.is_none());
    assert!(status.file_change_count.is_none());
}

#[test]
fn git_repository_reports_branch_and_head() {
    if !git_available() {
        eprintln!("skipping: git is not on PATH (mark #[ignore] on such a machine)");
        return;
    }
    let fixture = TempDir::new("repo");
    init_repo(fixture.path());

    let inspector = WorkspaceInspector::new();
    let status = inspector.status(&fixture.path().to_string_lossy());
    assert!(status.exists);
    assert!(status.is_git_repository);
    assert_eq!(status.branch.as_deref(), Some("main"));
    let head = status.head_sha.expect("head sha");
    assert_eq!(head.len(), 40, "unexpected sha: {head}");
    assert!(head.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(status.is_dirty, Some(false));
    assert_eq!(status.file_change_count, Some(0));
}

#[test]
fn untracked_file_marks_workspace_dirty() {
    if !git_available() {
        eprintln!("skipping: git is not on PATH (mark #[ignore] on such a machine)");
        return;
    }
    let fixture = TempDir::new("dirty");
    init_repo(fixture.path());
    std::fs::write(fixture.path().join("untracked.txt"), "new\n").expect("write untracked");

    let inspector = WorkspaceInspector::new();
    let status = inspector.status(&fixture.path().to_string_lossy());
    assert!(status.is_git_repository);
    assert_eq!(status.is_dirty, Some(true));
    assert_eq!(status.file_change_count, Some(1));
}

#[test]
fn diff_separates_staged_unstaged_and_untracked_files() {
    if !git_available() {
        eprintln!("skipping: git is not on PATH (mark #[ignore] on such a machine)");
        return;
    }
    let fixture = TempDir::new("diff");
    init_repo(fixture.path());
    std::fs::write(fixture.path().join("README.md"), "staged\n").expect("write staged file");
    git(fixture.path(), &["add", "README.md"]);
    std::fs::write(fixture.path().join("README.md"), "unstaged\n").expect("write unstaged file");
    std::fs::write(fixture.path().join("new.txt"), "new\n").expect("write untracked file");

    let diff = WorkspaceInspector::new()
        .diff(fixture.path())
        .expect("git diff");

    assert_eq!(diff.staged.len(), 1);
    assert_eq!(diff.unstaged.len(), 1);
    assert_eq!(diff.untracked.len(), 1);
    assert_eq!(diff.untracked[0].path, "new.txt");
}

#[test]
fn validate_rejects_missing_path() {
    let fixture = TempDir::new("validate-missing");
    let missing = fixture.path().join("nope");
    let inspector = WorkspaceInspector::new();
    let error = inspector
        .validate(&missing.to_string_lossy())
        .expect_err("missing path must be rejected");
    assert_eq!(error.code, ark_common::ArkErrorCode::ValidationError);
    assert!(error.message.contains("does not exist"));
}

#[test]
fn validate_rejects_file_and_empty_input() {
    let fixture = TempDir::new("validate-file");
    let file = fixture.path().join("plain.txt");
    std::fs::write(&file, "x\n").expect("write file");

    let inspector = WorkspaceInspector::new();
    let error = inspector
        .validate(&file.to_string_lossy())
        .expect_err("file must be rejected");
    assert!(error.message.contains("not a directory"));

    let empty = inspector
        .validate("   ")
        .expect_err("blank must be rejected");
    assert!(empty.message.contains("must not be empty"));
}

#[test]
fn validate_accepts_directory_and_expands_tilde() {
    let fixture = TempDir::new("validate-ok");
    let inspector = WorkspaceInspector::with_base(fixture.path());
    let resolved = inspector
        .validate(fixture.path().to_str().expect("utf-8 temp path"))
        .expect("existing directory should validate");
    assert_eq!(resolved, fixture.path());

    // `~` must expand to the host home directory, which always exists.
    let home = match crate::domain::paths::home_dir() {
        Some(home) => home,
        None => return,
    };
    let expanded = inspector.validate("~").expect("home should validate");
    assert_eq!(expanded, home);
}

#[test]
fn validate_resolves_relative_against_base() {
    let fixture = TempDir::new("validate-relative");
    let child = fixture.path().join("child");
    std::fs::create_dir(&child).expect("create child dir");

    let inspector = WorkspaceInspector::with_base(fixture.path());
    assert_eq!(inspector.validate("child").expect("relative"), child);
}

#[test]
fn inspector_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<WorkspaceInspector>();
}
