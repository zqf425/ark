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
pub const IGNORED_DIR_NAMES: [&str; 6] = [
    "target",
    "node_modules",
    "dist",
    ".git",
    ".tmp",
    ".worktrees",
];

/// Whether the walkers skip a directory entry with this name.
pub fn is_ignored_dir_name(name: &str) -> bool {
    IGNORED_DIR_NAMES.contains(&name)
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
            scan(&expired)
        });
        match tokio::time::timeout(deadline, handle).await {
            Ok(Ok(result)) => result,
            Ok(Err(join_err)) => Err(ArkError::new(
                ark_common::ArkErrorCode::ToolExecutionFailed,
                format!("background scan failed: {join_err}"),
            )),
            Err(_) => Err(ArkError::new(
                ark_common::ArkErrorCode::ToolExecutionFailed,
                format!(
                    "scan timed out after {} ms; narrow the search path or add a glob",
                    deadline.as_millis()
                ),
            )),
        }
    }
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
    fn errors_become_unsuccessful_output() {
        let output = as_output(Err(ArkError::validation("nope")));
        assert!(!output.ok);
        assert_eq!(output.content, "nope");
    }
}
