//! The tool-execution seam for the reactor (design §7.3).
//!
//! `ark-agent` may not depend on `ark-tool` (§5.2), so the reactor calls this
//! narrow trait. `ark-tool` implements it over the registry; tests use
//! [`MapToolRunner`].

use std::collections::BTreeMap;

use ark_common::Result;
use ark_ports::ToolOutput;

use crate::domain::tool::ToolCallRequest;

/// Execute exactly one tool call.
#[async_trait::async_trait]
pub trait ToolRunner: Send + Sync {
    /// Run one call. Returning `Err` is turned into a failed `ToolExecution` by
    /// the reactor, so a throwing runner can never leave a call unanswered.
    async fn execute(&self, call: &ToolCallRequest) -> Result<ToolOutput>;
}

/// Scripted runner keyed by `tool` name. Used by tests and by `--dry-run`.
#[derive(Debug, Clone, Default)]
pub struct MapToolRunner {
    responses: BTreeMap<String, ToolOutput>,
    fail_unknown: bool,
}

impl MapToolRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer every call of `tool` with `output`.
    pub fn with(mut self, tool: impl Into<String>, output: ToolOutput) -> Self {
        self.responses.insert(tool.into(), output);
        self
    }

    /// Make unknown tools fail instead of returning a placeholder.
    pub fn failing_unknown(mut self) -> Self {
        self.fail_unknown = true;
        self
    }
}

#[async_trait::async_trait]
impl ToolRunner for MapToolRunner {
    async fn execute(&self, call: &ToolCallRequest) -> Result<ToolOutput> {
        match self.responses.get(&call.tool) {
            Some(output) => Ok(output.clone()),
            None if self.fail_unknown => {
                Ok(ToolOutput::err(format!("unknown tool `{}`", call.tool)))
            }
            None => Ok(ToolOutput::ok(format!(
                "dry-run: {} {:?}",
                call.tool, call.arguments
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &str) -> ToolCallRequest {
        ToolCallRequest::new("c1", tool, serde_json::json!({ "path": "a" }))
    }

    #[tokio::test]
    async fn scripted_response_is_returned() {
        let runner = MapToolRunner::new().with("read", ToolOutput::ok("body"));
        let output = runner.execute(&call("read")).await.unwrap();
        assert!(output.ok);
        assert_eq!(output.content, "body");
    }

    #[tokio::test]
    async fn unknown_tools_get_a_placeholder_by_default() {
        let runner = MapToolRunner::new();
        let output = runner.execute(&call("bash")).await.unwrap();
        assert!(output.ok);
        assert!(output.content.contains("dry-run"));
    }

    #[tokio::test]
    async fn unknown_tools_can_fail_instead() {
        let runner = MapToolRunner::new().failing_unknown();
        let output = runner.execute(&call("bash")).await.unwrap();
        assert!(!output.ok);
    }
}
