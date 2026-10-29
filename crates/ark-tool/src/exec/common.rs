//! Shared plumbing for the builtin tools (design §7.3).
//!
//! One place decides how a tool reports a failure and how it resolves a path,
//! so the seven implementations stay short and behave identically.

use ark_common::{ArkError, Result};
use ark_ports::{SandboxDecision, ToolContext, ToolOutput};
use std::path::PathBuf;

use crate::domain::paths;

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
