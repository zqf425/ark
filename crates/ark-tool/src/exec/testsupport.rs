//! Shared test fixtures for the builtin tool suites.

use ark_common::{ThreadId, TurnId};
use ark_ports::{SandboxDecision, Tool, ToolArgs, ToolContext, ToolOutput};

use crate::exec::tools::bash::BashTool;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Fresh, empty workspace directory named after the test.
pub(crate) fn temp_workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ark-tool-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp workspace");
    dir
}

/// Context rooted at `root` with no environment overrides.
pub(crate) fn ctx(root: &Path) -> ToolContext {
    ToolContext {
        thread_id: ThreadId::new(),
        turn_id: TurnId::new(),
        workspace: root.to_path_buf(),
        sandbox: SandboxDecision::ReadOnly,
        env: BTreeMap::new(),
    }
}

/// Run `count` shells of `command` the way the reactor does: every call is
/// spawned at once and awaited together, and the wall clock covers the batch.
/// Used by the parallel-shell regression test.
pub(crate) async fn time_parallel_bash(
    root: &Path,
    count: usize,
    command: &str,
) -> (std::time::Duration, Vec<ToolOutput>) {
    let started = std::time::Instant::now();
    let mut handles = Vec::with_capacity(count);
    for _ in 0..count {
        let ctx = ctx(root);
        let command = command.to_string();
        handles.push(tokio::spawn(async move {
            BashTool
                .execute(&ctx, ToolArgs(serde_json::json!({ "command": command })))
                .await
                .unwrap()
        }));
    }
    let mut outputs = Vec::with_capacity(count);
    for handle in handles {
        outputs.push(handle.await.expect("shell task panicked"));
    }
    (started.elapsed(), outputs)
}
