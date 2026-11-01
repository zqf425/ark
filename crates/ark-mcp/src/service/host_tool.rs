//! Host-side view of a plugin tool: the [`Tool`] the agent loop sees.
//!
//! A plugin publishes `<plugin_id>.<tool>` to [`McpBus`], and this adapter is
//! what puts that name into the model's tool catalogue. It is deliberately thin:
//! every rule (namespace, depth, cycle, deadlock, handler errors) is already
//! enforced by the bus, so the adapter only forwards and renders.
//!
//! ## Why the names are leaked
//!
//! [`Tool::name`] and [`Tool::schema`] hand out `'static` references, because a
//! schema that changed after registration would silently break the prompt cache
//! prefix. Both are leaked once per published tool, matching the budget the
//! external `McpToolAdapter` already documents.
//!
//! ## Host ingress is not a plugin
//!
//! The agent loop calls a plugin tool on behalf of the *user*, not on behalf of
//! another plugin. It therefore has no `mcp.consume` list to satisfy, and the
//! call goes through [`McpBus::dispatch_from_host`] instead of the
//! plugin-to-plugin `dispatch`. Namespace and recursion guards still apply.

use ark_common::Result;
use ark_ports::{Tool, ToolArgs, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::domain::namespace::{ensure_local, qualify, validate_plugin_id};
use crate::service::bus::McpBus;

/// One plugin tool, callable from the agent loop.
pub struct PluginTool {
    /// `<plugin_id>.<tool>`, leaked once at construction.
    name: &'static str,
    /// Canonical schema, leaked once at construction.
    schema: &'static Value,
    /// Plugin that serves the tool.
    provider: String,
    /// Local tool name, without the namespace.
    tool: String,
    bus: Arc<McpBus>,
}

impl std::fmt::Debug for PluginTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginTool")
            .field("name", &self.name)
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl PluginTool {
    /// Build an adapter for one published tool.
    ///
    /// Returns an error for a name the bus would not accept, so a bad manifest
    /// fails at load time instead of when the model first calls the tool.
    pub fn new(bus: Arc<McpBus>, provider: &str, tool: &str, schema: Value) -> Result<Self> {
        validate_plugin_id(provider)?;
        ensure_local(provider, tool)?;
        let schema = if schema.is_object() {
            schema
        } else {
            serde_json::json!({ "type": "object", "properties": {} })
        };
        let name: &'static str = Box::leak(qualify(provider, tool).into_boxed_str());
        let schema: &'static Value = Box::leak(Box::new(schema));
        Ok(Self {
            name,
            schema,
            provider: provider.to_string(),
            tool: tool.to_string(),
            bus,
        })
    }

    /// Container plugin that serves this tool.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Local tool name, as the plugin knows it.
    pub fn local_name(&self) -> &str {
        &self.tool
    }
}

#[async_trait]
impl Tool for PluginTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn schema(&self) -> &'static Value {
        self.schema
    }

    async fn execute(&self, _ctx: &ToolContext, args: ToolArgs) -> Result<ToolOutput> {
        let arguments = if args.0.is_null() {
            serde_json::json!({})
        } else {
            args.0
        };
        match self
            .bus
            .dispatch_from_host(&self.provider, &self.tool, arguments)
            .await
        {
            Ok(result) => Ok(render(result)),
            Err(err) => Ok(ToolOutput {
                ok: false,
                content: format!("plugin tool {} failed: {}", self.name, err.message),
                detail: err.detail.clone(),
            }),
        }
    }
}

/// Bus payload as tool text. A JSON object is pretty-printed; a plain string is
/// passed through so the model sees what the plugin wrote, not a quoted copy.
fn render(result: ark_ports::McpCallResult) -> ToolOutput {
    let content = match result.content {
        Value::String(text) => text,
        Value::Null => String::new(),
        other => serde_json::to_string_pretty(&other).unwrap_or_else(|_| other.to_string()),
    };
    let content = if content.trim().is_empty() {
        format!(
            "(plugin tool returned no content, trace {})",
            result.trace_id
        )
    } else {
        content
    };
    ToolOutput {
        ok: result.ok,
        content,
        detail: if result.trace_id.is_empty() {
            None
        } else {
            Some(result.trace_id)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::McpHandler;
    use ark_ports::{McpCallResult, McpToolSpec};

    struct Echo(&'static str);

    #[async_trait]
    impl McpHandler for Echo {
        async fn call(
            &self,
            _ctx: &crate::service::CallContext,
            args: Value,
        ) -> Result<McpCallResult> {
            Ok(McpCallResult {
                ok: true,
                content: serde_json::json!({ "echo": args, "label": self.0 }),
                trace_id: String::new(),
            })
        }
    }

    fn bus_with_tool(plugin: &str, tool: &str) -> Arc<McpBus> {
        let bus = Arc::new(McpBus::default());
        let spec = McpToolSpec {
            plugin_id: plugin.to_string(),
            tool: tool.to_string(),
            description: "echo".to_string(),
            schema: serde_json::json!({ "type": "object", "properties": {} }),
        };
        bus.expose_with_handlers(plugin, vec![(spec, Arc::new(Echo("hi")))])
            .unwrap()
            .forget();
        bus
    }

    #[test]
    fn a_plugin_tool_is_named_with_the_bus_namespace() {
        let bus = bus_with_tool("ark.example.hello", "greet");
        let tool = PluginTool::new(
            bus,
            "ark.example.hello",
            "greet",
            serde_json::json!({ "type": "object" }),
        )
        .unwrap();
        assert_eq!(tool.name(), "ark.example.hello.greet");
        assert!(tool.schema().is_object());
    }

    #[test]
    fn a_tool_without_a_schema_still_gets_an_object_schema() {
        let bus = bus_with_tool("ark.example.hello", "greet");
        let tool = PluginTool::new(bus, "ark.example.hello", "greet", Value::Null).unwrap();
        assert_eq!(tool.schema()["type"], "object");
    }

    #[tokio::test]
    async fn a_host_call_reaches_the_plugin_without_a_visibility_declaration() {
        let bus = bus_with_tool("ark.example.hello", "greet");
        let tool = PluginTool::new(
            Arc::clone(&bus),
            "ark.example.hello",
            "greet",
            serde_json::json!({}),
        )
        .unwrap();
        let ctx = ToolContext {
            thread_id: ark_common::ThreadId::new(),
            turn_id: ark_common::TurnId::new(),
            workspace: std::path::PathBuf::from("."),
            sandbox: ark_ports::SandboxDecision::ReadOnly,
            env: Default::default(),
        };
        let output = tool
            .execute(&ctx, ToolArgs(serde_json::json!({ "name": "ark" })))
            .await
            .unwrap();
        assert!(output.ok);
        assert!(output.content.contains("\"ark\""));
    }

    #[tokio::test]
    async fn a_bus_error_becomes_a_tool_error_not_a_harness_failure() {
        let bus = bus_with_tool("ark.example.hello", "greet");
        let tool = PluginTool::new(
            Arc::clone(&bus),
            "ark.example.hello",
            "greet",
            serde_json::json!({}),
        )
        .unwrap();
        // The plugin is unloaded, so the bus can no longer resolve the tool.
        bus.unregister_plugin("ark.example.hello");

        let ctx = ToolContext {
            thread_id: ark_common::ThreadId::new(),
            turn_id: ark_common::TurnId::new(),
            workspace: std::path::PathBuf::from("."),
            sandbox: ark_ports::SandboxDecision::ReadOnly,
            env: Default::default(),
        };
        let output = tool
            .execute(&ctx, ToolArgs(serde_json::json!({})))
            .await
            .unwrap();
        assert!(!output.ok);
        assert!(output.content.contains("is not registered"));
    }
}
