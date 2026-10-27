//! `ToolRunner` adapter (design §7.3 / §7.1).
//!
//! `ark-agent` may not depend on `ark-tool`, so the reactor receives this thin
//! adapter. The sandbox decision is computed **once** here, before dispatch, and
//! travels inside `ToolContext`: no tool re-derives policy.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use ark_common::Result;
use ark_ports::{SandboxDecision, SandboxPolicy, ToolArgs, ToolContext, ToolOutput};

use crate::service::registry::ToolRegistry;

/// Executes one tool call against the registry.
///
/// `ark-agent`'s `ToolCallRequest` is structurally identical to
/// `(call_id, tool, arguments)`, so this struct exposes a plain `run` method and
/// `arkd` adapts it to the reactor's trait without a second shape.
pub struct RegistryToolRunner {
    registry: Arc<ToolRegistry>,
    policy: Arc<dyn SandboxPolicy>,
    workspace: PathBuf,
    env: BTreeMap<String, String>,
}

impl RegistryToolRunner {
    pub fn new(
        registry: Arc<ToolRegistry>,
        policy: Arc<dyn SandboxPolicy>,
        workspace: PathBuf,
    ) -> Self {
        Self {
            registry,
            policy,
            workspace,
            env: BTreeMap::new(),
        }
    }

    /// Attach an immutable environment snapshot (design §7.9: no `process::env`).
    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Decision for one call, computed before execution.
    pub fn decide(&self, tool: &str, args: &ToolArgs) -> SandboxDecision {
        self.policy.decide(tool, args)
    }

    /// Run one call. A `NeedsApproval` decision is refused here, because the
    /// approval gate lives in the agent loop: reaching this point with it means
    /// the call bypassed the gate.
    pub async fn run(
        &self,
        thread_id: ark_common::ThreadId,
        turn_id: ark_common::TurnId,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolOutput> {
        let workspace = self.workspace.clone();
        self.run_in(workspace, thread_id, turn_id, tool, arguments)
            .await
    }

    /// Run one call against an explicit workspace root.
    ///
    /// `arkd` uses this so a workspace chosen in Settings applies to the very
    /// next tool call: the root is resolved per call from the live config
    /// snapshot instead of being frozen at process start.
    pub async fn run_in(
        &self,
        workspace: PathBuf,
        thread_id: ark_common::ThreadId,
        turn_id: ark_common::TurnId,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolOutput> {
        let args = ToolArgs(arguments);
        let sandbox = self.decide(tool, &args);
        if sandbox == SandboxDecision::NeedsApproval {
            return Ok(ToolOutput::err(format!(
                "tool `{tool}` requires approval and was not dispatched"
            )));
        }
        let ctx = ToolContext {
            thread_id,
            turn_id,
            workspace,
            sandbox,
            env: self.env.clone(),
        };
        self.registry.execute(tool, &ctx, args).await
    }
}

/// Adapter to the `ark-agent` runner shape, so `arkd` wires one object.
pub struct AgentToolRunner {
    inner: Arc<RegistryToolRunner>,
    thread_id: ark_common::ThreadId,
    turn_id: ark_common::TurnId,
}

impl AgentToolRunner {
    pub fn new(
        inner: Arc<RegistryToolRunner>,
        thread_id: ark_common::ThreadId,
        turn_id: ark_common::TurnId,
    ) -> Self {
        Self {
            inner,
            thread_id,
            turn_id,
        }
    }
}

impl AgentToolRunner {
    /// Run one call bound to the `(thread, turn)` pair captured at construction.
    ///
    /// `ark-tool` may not depend on `ark-agent`, so `arkd` wraps this in its own
    /// `ToolRunner` impl rather than the reverse.
    pub async fn execute(&self, tool: &str, arguments: serde_json::Value) -> Result<ToolOutput> {
        self.inner
            .run(self.thread_id, self.turn_id, tool, arguments)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::tools::builtins;
    use crate::policy::Autonomy;
    use crate::policy::BuiltinSandboxPolicy;
    use crate::service::registry::ToolRegistry;

    fn runner(dir: &std::path::Path) -> RegistryToolRunner {
        let mut registry = ToolRegistry::new();
        registry.register_all(builtins()).unwrap();
        RegistryToolRunner::new(
            Arc::new(registry),
            Arc::new(BuiltinSandboxPolicy::new(Autonomy::WorkspaceWrite)),
            dir.to_path_buf(),
        )
    }

    #[tokio::test]
    async fn read_runs_unattended() {
        let dir = crate::exec::testsupport::temp_workspace("runner-read");
        std::fs::write(dir.join("a.txt"), "hi\n").unwrap();
        let out = runner(&dir)
            .run(
                ark_common::ThreadId::new(),
                ark_common::TurnId::new(),
                "read",
                serde_json::json!({ "path": "a.txt" }),
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.content.contains("hi"));
    }

    #[tokio::test]
    async fn bash_is_refused_when_the_policy_asks_for_approval() {
        let dir = crate::exec::testsupport::temp_workspace("runner-bash");
        let out = runner(&dir)
            .run(
                ark_common::ThreadId::new(),
                ark_common::TurnId::new(),
                "bash",
                serde_json::json!({ "command": "echo hi" }),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("requires approval"));
    }

    #[tokio::test]
    async fn unknown_tool_is_refused_before_dispatch() {
        let dir = crate::exec::testsupport::temp_workspace("runner-unknown");
        // Unknown tools are classified as `NeedsApproval` by the policy, so the
        // runner refuses them here instead of reaching the registry lookup.
        let out = runner(&dir)
            .run(
                ark_common::ThreadId::new(),
                ark_common::TurnId::new(),
                "nope",
                serde_json::json!({}),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.content.contains("requires approval"));
    }

    #[tokio::test]
    async fn unattended_mode_still_reports_an_unknown_tool_as_an_error() {
        let dir = crate::exec::testsupport::temp_workspace("runner-unknown-unattended");
        let mut registry = ToolRegistry::new();
        registry.register_all(builtins()).unwrap();
        let runner = RegistryToolRunner::new(
            Arc::new(registry),
            Arc::new(BuiltinSandboxPolicy::new(Autonomy::Unattended)),
            dir.to_path_buf(),
        );
        let result = runner
            .run(
                ark_common::ThreadId::new(),
                ark_common::TurnId::new(),
                "nope",
                serde_json::json!({}),
            )
            .await;
        assert!(result.is_err());
    }
}
