//! Tool port (design §7.3).
//!
//! Sandboxing is decided *once* before dispatch and handed to the tool, instead
//! of every tool re-deriving its own policy.

use ark_common::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Strongly typed arguments. The registry deserializes against the tool schema
/// so a tool never parses a JSON string by hand.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolArgs(pub serde_json::Value);

impl ToolArgs {
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.as_str())
    }
}

/// Everything a tool may touch. `env` is an immutable snapshot: connector
/// variables are never written into the process environment.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub thread_id: ark_common::ThreadId,
    pub turn_id: ark_common::TurnId,
    pub workspace: PathBuf,
    pub sandbox: SandboxDecision,
    pub env: std::collections::BTreeMap<String, String>,
}

/// Result of one tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub ok: bool,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            ok: true,
            content: content.into(),
            detail: None,
        }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self {
            ok: false,
            content: content.into(),
            detail: None,
        }
    }
}

/// How much a tool is allowed to do, computed before dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxDecision {
    /// Read-only operations inside the workspace.
    ReadOnly,
    /// Writes confined to the workspace root.
    WorkspaceWrite,
    /// Requires an approval gate before execution.
    NeedsApproval,
}

/// Policy that turns a request into a `SandboxDecision`. One implementation,
/// consulted once per dispatch.
pub trait SandboxPolicy: Send + Sync {
    fn decide(&self, tool: &str, args: &ToolArgs) -> SandboxDecision;
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;

    /// Canonical JSON schema, sorted once at registration so the prompt cache
    /// prefix stays byte-stable between turns.
    fn schema(&self) -> &'static serde_json::Value;

    async fn execute(&self, ctx: &ToolContext, args: ToolArgs) -> Result<ToolOutput>;
}

/// Read-only view of the tool catalogue that the agent depends on.
pub trait ToolRegistryPort: Send + Sync {
    fn names(&self) -> Vec<String>;

    /// Schemas in canonical order, ready to embed in a `ModelRequest`.
    fn schemas(&self) -> Vec<serde_json::Value>;

    /// Monotonic version; a change invalidates the cached prompt prefix.
    fn version(&self) -> u64;
}
