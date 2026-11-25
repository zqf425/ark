//! Shared plumbing for the builtin tools (design §7.3).
//!
//! One place decides how a tool reports a failure and how it resolves a path,
//! so the seven implementations stay short and behave identically.

use ark_common::{ArkError, Result};
use ark_ports::{SandboxDecision, ToolContext, ToolOutput};
use std::path::PathBuf;
use std::time::Duration;

use crate::domain::paths;
use crate::policy::truncate::{clamp_timeout, DEFAULT_LIMITS};

/// Directory names the recursive walkers (`find`, `grep`) never enter.
///
/// The list covers build outputs and dependency caches (`target`,
/// `node_modules`, `dist`), version-control objects (`.git`) and the scratch
/// layout every worktree and agent task uses (`.tmp`, `.worktrees`, per
/// AGENTS.md §2/§7.1). These dwarf real sources: before the list existed, one
/// grep over this workspace walked ~380k files for eleven minutes (session
/// 936001de, two calls, 1005 s). Names are judged on every directory **below**
/// the search root, so an explicit `path` into an ignored directory still
/// scans it deliberately.
///
/// The rest of the list is the same class of high-volume, low-signal
/// directory: language package caches, build systems, editor state and
/// virtualenvs. A Go/Python or Xcode tree without these was one unlisted
/// `vendor`/`__pycache__`/`Pods` away from repeating the 410k-file walk.
/// `site-packages` stays out on purpose: a global interpreter dir is not
/// inside a repo, and a virtualenv is already covered by `.venv`/`venv`.
pub const IGNORED_DIR_NAMES: [&str; 21] = [
    "target",
    "node_modules",
    "dist",
    ".git",
    ".tmp",
    ".worktrees",
    // package managers & language caches
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    // build systems & outputs
    "build",
    "out",
    ".next",
    ".nuxt",
    "DerivedData",
    "Pods",
    ".gradle",
    // editor / tooling state
    ".idea",
    ".vscode",
    ".cache",
    ".sass-cache",
];

/// Whether the walkers skip a directory entry with this name.
pub fn is_ignored_dir_name(name: &str) -> bool {
    IGNORED_DIR_NAMES.contains(&name)
}

/// Whether a traversed entry still resolves inside the search root.
///
/// Shared by the `find` and `grep` walkers. Unix checks directories only:
/// the walks never follow symlinks, `file_type()` comes from the read_dir
/// dirent rather than a stat, and a file's parent directory has already been
/// verified in-root, so a file entry cannot escape without a directory in
/// its chain having been rejected. Rewriting `is_dir` from the entry's
/// `FileType` to `path.is_dir()` (stat semantics) would silently drop that
/// premise. Windows checks every entry: junctions are reparse points without
/// the name-surrogate bit, so `FileType::is_symlink()` returns false for
/// them and this check is the only line of defense (design ark-tool.md:15).
pub fn entry_inside_root(
    is_dir: bool,
    path: &std::path::Path,
    real_root: &std::path::Path,
) -> bool {
    if !cfg!(windows) && !is_dir {
        return true;
    }
    match std::fs::canonicalize(path) {
        Ok(real_path) => real_path.starts_with(real_root),
        Err(_) => false,
    }
}

/// Resolve a tool-supplied path inside the workspace, or fail with validation.
pub fn workspace_path(ctx: &ToolContext, input: Option<&str>) -> Result<PathBuf> {
    match input {
        Some(value) if !value.trim().is_empty() => resolve_path(ctx, value.trim()),
        Some(_) | None => resolve_path(ctx, "."),
    }
}

fn resolve_path(ctx: &ToolContext, input: &str) -> Result<PathBuf> {
    match ctx.sandbox {
        SandboxDecision::Unrestricted => paths::resolve_unrestricted(&ctx.workspace, input),
        _ => paths::resolve(&ctx.workspace, input),
    }
}

/// Turn a tool-level error into an `ok: false` output instead of a panic.
///
/// The reactor treats `Err` as a failed call, but a tool that already produced
/// a useful message should report it as content so the model can recover.
pub fn as_output(result: Result<ToolOutput>) -> ToolOutput {
    match result {
        Ok(output) => output,
        Err(err) => ToolOutput::err(err.message),
    }
}

/// Read a file through the single encoding policy (`ark_common::encoding`).
///
/// Windows Notepad and old PowerShell write UTF-8 *with* a BOM, and a Chinese
/// Windows host can still produce GBK. Tools must not decide this on their own:
/// `read`, `edit` and `grep` all go through here so the rule cannot drift.
pub fn read_text(path: &std::path::Path) -> Result<String> {
    ark_common::read_text(path).map_err(|err| {
        ArkError::new(
            ark_common::ArkErrorCode::ToolExecutionFailed,
            format!("cannot read {}: {}", path.display(), err.message),
        )
    })
}

/// Write text as UTF-8 **without a BOM**, creating parent directories.
///
/// A model can echo a BOM back into its `write`/`edit` content. The writer strips
/// it instead of propagating it into the user's repository.
pub fn write_text(path: &std::path::Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|err| ArkError::from(std::io::Error::new(err.kind(), err)))?;
        }
    }
    ark_common::write_text(path, text)
}

/// Decode one child process' captured output stream as text.
///
/// Child output is bytes, not UTF-8: a Chinese Windows host runs `cmd`,
/// `mvn`, `python`, `tasklist` and friends under the active ANSI code page
/// (GBK/CP936), and those bytes are not valid UTF-8. `String::from_utf8_lossy`
/// turned every such line into U+FFFD, so a "command not found" report arrived
/// as unreadable replacement characters and the reason was lost — 19 of 111
/// failed `bash` calls in one day's sessions carried replacement characters, and
/// 99 more "successful" ones were silently garbled. Decoding goes through the
/// project's single encoding policy (`ark_common::decode`: strict UTF-8 first,
/// then GB18030/Big5/Shift-JIS/EUC-KR, Latin-1 as the always-succeeds last
/// resort), so shell output and file reads share one rule instead of drifting.
///
/// NUL bytes are kept as-is: a command that emits them is reporting binary
/// data, and silently stripping the terminator would misreport the content.
pub fn decode_shell_output(bytes: &[u8]) -> String {
    decode_shell_output_status(bytes).text
}

/// Decode shell output while retaining which shared encoding policy matched.
pub(crate) fn decode_shell_output_status(bytes: &[u8]) -> ark_common::DecodedText {
    ark_common::decode(bytes)
}

/// Attach a detail string to an output.
///
/// `ark-ports` is frozen and `ToolOutput` has no builder, so the helper lives
/// here rather than as a second constructor in the port crate.
pub fn with_detail(mut output: ToolOutput, detail: impl Into<String>) -> ToolOutput {
    output.detail = Some(detail.into());
    output
}

/// Run a synchronous directory scan on the blocking pool with the bash timeout
/// budget.
///
/// `grep` and `find` are plain `std::fs` walks; inlining them into the async
/// executor blocked a tokio worker for the whole scan, which both starved the
/// runtime (other turns queued behind it) and made the turn's cancellation
/// unpollable — a user pressing "stop" could not interrupt an eleven-minute
/// grep. `spawn_blocking` moves the walk off the workers; the deadline is the
/// same clamped budget `bash` already uses, so every long-running builtin
/// shares one bound. `handle.is_expired()` lets the inner loop stop early
/// instead of burning the pool until the timeout fires.
pub(crate) struct BlockingScan {
    deadline: Duration,
}

impl BlockingScan {
    pub(crate) fn start() -> Self {
        Self {
            deadline: Duration::from_millis(clamp_timeout(None, &DEFAULT_LIMITS)),
        }
    }

    /// Run `scan` off the async workers; a deadline overrun becomes a
    /// structured error naming the budget, mirroring the bash timeout detail.
    pub(crate) async fn run<T, F>(&self, scan: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&ScanDeadline) -> Result<T> + Send + 'static,
    {
        let deadline = self.deadline;
        let started = std::time::Instant::now();
        let handle = tokio::task::spawn_blocking(move || {
            let expired = ScanDeadline { started, deadline };
            let result = scan(&expired);
            // A cooperative walker may notice the deadline and return its
            // partial accumulator just before the outer timeout wins the race.
            // That is still a timeout, never a successful complete scan.
            expired.check()?;
            result
        });
        match tokio::time::timeout(deadline, handle).await {
            Ok(Ok(result)) => result,
            Ok(Err(join_err)) => Err(ArkError::new(
                ark_common::ArkErrorCode::ToolExecutionFailed,
                format!("background scan failed: {join_err}"),
            )),
            Err(_) => Err(scan_timeout(deadline)),
        }
    }
}

fn scan_timeout(deadline: Duration) -> ArkError {
    ArkError::new(
        ark_common::ArkErrorCode::ToolExecutionFailed,
        format!(
            "scan timed out after {} ms; narrow the search path or add a glob",
            deadline.as_millis()
        ),
    )
}

/// Cooperative deadline the inner walk checks between entries.
pub(crate) struct ScanDeadline {
    started: std::time::Instant,
    deadline: Duration,
}

impl ScanDeadline {
    /// True once the shared scan budget is spent; checked per directory and
    /// per file so a huge tree stops within one entry of the deadline.
    pub(crate) fn is_expired(&self) -> bool {
        self.started.elapsed() >= self.deadline
    }

    /// Convert an expired cooperative scan into the same structured failure as
    /// the outer timeout, including when the walker already returned partial data.
    pub(crate) fn check(&self) -> Result<()> {
        if self.is_expired() {
            Err(scan_timeout(self.deadline))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::{ThreadId, TurnId};
    use ark_ports::SandboxDecision;
    use std::collections::BTreeMap;

    fn ctx(root: &std::path::Path) -> ToolContext {
        ToolContext {
            thread_id: ThreadId::new(),
            turn_id: TurnId::new(),
            workspace: root.to_path_buf(),
            sandbox: SandboxDecision::ReadOnly,
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn missing_path_defaults_to_the_root() {
        let dir = std::env::temp_dir();
        let resolved = workspace_path(&ctx(&dir), None).unwrap();
        assert!(resolved.exists());
    }

    #[test]
    fn escapes_are_refused() {
        let dir = std::env::temp_dir().join("ws");
        assert!(workspace_path(&ctx(&dir), Some("../../x")).is_err());
    }

    #[test]
    fn unrestricted_mode_allows_a_valid_path_outside_the_workspace() {
        let dir = std::env::temp_dir().join("ws");
        let mut context = ctx(&dir);
        context.sandbox = SandboxDecision::Unrestricted;
        assert!(workspace_path(&context, Some("C:/Windows/System32")).is_ok());
    }

    #[test]
    fn expired_scan_rejects_partial_results() {
        let deadline = ScanDeadline {
            started: std::time::Instant::now() - Duration::from_secs(1),
            deadline: Duration::from_secs(1),
        };
        let partial = vec!["partial result"];
        let result = deadline.check().map(|()| partial);
        assert!(result.is_err(), "an expired scan must not report success");
    }

    #[test]
    fn active_scan_can_return_a_complete_result() {
        let deadline = ScanDeadline {
            started: std::time::Instant::now(),
            deadline: Duration::from_secs(60),
        };
        assert!(deadline.check().is_ok());
    }

    #[test]
    fn errors_become_unsuccessful_output() {
        let output = as_output(Err(ArkError::validation("nope")));
        assert!(!output.ok);
        assert_eq!(output.content, "nope");
    }
}
