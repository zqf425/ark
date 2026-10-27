//! [`McpToolAdapter`]: one remote MCP tool exposed as a local [`Tool`].
//!
//! Tools from external servers join the same tool catalogue as builtins, so the
//! agent loop never learns where a tool lives. That requires two `'static`
//! values, because [`Tool`] hands out references with the process lifetime:
//!
//! * `name()` must be `&'static str`;
//! * `schema()` must be `&'static serde_json::Value`, and the prompt cache
//!   prefix is only stable if the value never changes after registration.
//!
//! Both are produced with [`Box::leak`] exactly once, when the adapter is
//! built. **Leak budget:** `tools × (name + schema)`, a one-time allocation for
//! a process that lives as long as the desktop app. A server whose catalogue
//! changes is reconnected through the registry, which builds fresh adapters and
//! leaves the old, small allocations behind; that is acceptable because the
//! number of configured servers is user-bounded (design 7.6.3).
//!
//! ## Error policy
//!
//! A remote JSON-RPC error is *not* a harness failure. It is returned as
//! [`ToolOutput::err`] so the model sees the server's message and can correct
//! its arguments, exactly like a local tool validation error.

use crate::service::servers::session::ServerSession;
use ark_common::Result;
use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// Upper bound for one remote call, so a wedged server cannot stall a turn.
pub(crate) const CALL_TIMEOUT_MS: u64 = 60_000;

/// Schema used when a server omits `inputSchema`.
fn fallback_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": true })
}

/// One remote tool.
pub struct McpToolAdapter {
    /// `<server>.<tool>`, leaked once at construction.
    name: &'static str,
    /// Canonical schema, leaked once at construction.
    schema: &'static Value,
    /// Tool name as the remote server knows it, without the namespace.
    remote: String,
    /// Server id, kept for error messages.
    server: String,
    session: Arc<ServerSession>,
}

impl std::fmt::Debug for McpToolAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpToolAdapter")
            .field("name", &self.name)
            .field("remote", &self.remote)
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

impl McpToolAdapter {
    /// Build an adapter for one tool of an already connected server.
    pub(crate) fn new(
        server: &str,
        remote: &str,
        schema: Option<Value>,
        session: Arc<ServerSession>,
    ) -> Self {
        let name: &'static str = Box::leak(format!("{server}.{remote}").into_boxed_str());
        let schema: &'static Value = Box::leak(Box::new(
            schema
                .filter(Value::is_object)
                .unwrap_or_else(fallback_schema),
        ));
        Self {
            name,
            schema,
            remote: remote.to_string(),
            server: server.to_string(),
            session,
        }
    }

    /// Server id this tool came from.
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Tool name as the remote server knows it.
    pub fn remote_name(&self) -> &str {
        &self.remote
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn schema(&self) -> &'static Value {
        self.schema
    }

    async fn execute(&self, _ctx: &ToolContext, args: ToolArgs) -> Result<ToolOutput> {
        let arguments = if args.0.is_null() { json!({}) } else { args.0 };
        let call = self.session.call_tool(&self.remote, arguments);
        match tokio::time::timeout(Duration::from_millis(CALL_TIMEOUT_MS), call).await {
            Ok(Ok(result)) => Ok(render(result)),
            Ok(Err(err)) => Ok(ToolOutput::err(format!(
                "mcp server {} tool {} failed: {}",
                self.server, self.remote, err.message
            ))),
            Err(_) => Ok(ToolOutput::err(format!(
                "mcp server {} tool {} timed out after {}ms",
                self.server, self.remote, CALL_TIMEOUT_MS
            ))),
        }
    }
}

/// Remote `isError` becomes a tool error; otherwise the flattened text is the
/// result. An empty payload still carries a readable placeholder so the model
/// never sees a blank tool result.
fn render(result: crate::domain::jsonrpc::CallToolResult) -> ToolOutput {
    let text = result.joined_text();
    let text = if text.trim().is_empty() {
        "(mcp server returned no text content)".to_string()
    } else {
        text
    };
    if result.is_error {
        ToolOutput::err(text)
    } else {
        ToolOutput::ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::jsonrpc::{CallToolResult, ContentBlock};

    #[test]
    fn empty_payload_is_not_a_blank_result() {
        let output = render(CallToolResult::default());
        assert!(output.ok);
        assert!(output.content.contains("no text content"));
    }

    #[test]
    fn remote_is_error_flag_maps_to_tool_error() {
        let mut result = CallToolResult::text("bad argument: jql");
        result.is_error = true;
        let output = render(result);
        assert!(!output.ok);
        assert_eq!(output.content, "bad argument: jql");
    }

    #[test]
    fn multiple_text_blocks_are_joined() {
        let result = CallToolResult {
            content: vec![
                ContentBlock::Text {
                    text: "line 1".into(),
                },
                ContentBlock::Text {
                    text: "line 2".into(),
                },
            ],
            is_error: false,
        };
        assert_eq!(render(result).content, "line 1\nline 2");
    }

    #[test]
    fn fallback_schema_is_an_object() {
        assert_eq!(fallback_schema()["type"], "object");
        assert!(fallback_schema()["properties"].is_object());
    }
}
