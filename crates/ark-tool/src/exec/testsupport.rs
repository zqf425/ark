//! Shared test fixtures for the builtin tool suites.

use ark_common::{ThreadId, TurnId};
use ark_ports::{SandboxDecision, ToolContext};
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
