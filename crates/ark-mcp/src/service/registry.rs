//! Tool registry for the single MCP egress (design 7.5.2).
//!
//! Registration is the only place a tool name can appear, so namespace
//! conflicts fail here rather than silently shadowing another plugin's tool.

use crate::domain::call_chain::CallFrame;
use crate::domain::namespace::{ensure_local, qualify};
use ark_common::{ArkError, Result};
use ark_ports::{McpCallResult, McpToolSpec};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

/// Everything a handler needs to answer one call.
#[derive(Debug, Clone)]
pub struct CallContext {
    /// Plugin that issued the call.
    pub caller: String,
    /// Plugin serving the tool.
    pub provider: String,
    /// Local tool name (without the namespace prefix).
    pub tool: String,
    /// Trace id of the ingress call, propagated through the whole chain.
    pub trace_id: String,
    /// Depth of this frame inside the chain (1 = first nested call).
    pub depth: usize,
    /// Frames active before this one, outermost first.
    pub chain: Vec<String>,
}

/// A plugin-owned tool implementation.
///
/// Handlers run in-process; the bus never opens a socket for them.
#[async_trait]
pub trait McpHandler: Send + Sync {
    async fn call(&self, ctx: &CallContext, args: serde_json::Value) -> Result<McpCallResult>;
}

/// One registered tool: public spec plus the handler.
#[derive(Clone)]
pub struct RegisteredTool {
    pub spec: McpToolSpec,
    pub handler: Arc<dyn McpHandler>,
    /// Exposure group that owns this entry (for scoped deregistration).
    pub exposure: String,
}

impl std::fmt::Debug for RegisteredTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredTool")
            .field("spec", &self.spec)
            .field("exposure", &self.exposure)
            .finish_non_exhaustive()
    }
}

/// The registry itself. Shared behind a lock by [`crate::service::McpBus`].
#[derive(Debug, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
    /// exposure id -> qualified tool names it contributed.
    exposures: BTreeMap<String, Vec<String>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one tool. Fails on a duplicate qualified name or an invalid
    /// name, and never partially applies a batch.
    pub fn insert(
        &mut self,
        exposure: &str,
        spec: McpToolSpec,
        handler: Arc<dyn McpHandler>,
    ) -> Result<()> {
        ensure_local(&spec.plugin_id, &spec.tool)?;
        let key = qualify(&spec.plugin_id, &spec.tool);
        if let Some(existing) = self.tools.get(&key) {
            return Err(ArkError::conflict(format!(
                "mcp tool {key} is already registered by exposure {}",
                existing.exposure
            ))
            .with_detail("mcp_tool_conflict"));
        }
        self.tools.insert(
            key.clone(),
            RegisteredTool {
                spec,
                handler,
                exposure: exposure.to_string(),
            },
        );
        self.exposures
            .entry(exposure.to_string())
            .or_default()
            .push(key);
        Ok(())
    }

    pub fn get(&self, provider: &str, tool: &str) -> Option<&RegisteredTool> {
        self.tools.get(&qualify(provider, tool))
    }

    /// Remove everything one exposure contributed. Idempotent.
    pub fn remove_exposure(&mut self, exposure: &str) -> usize {
        let Some(keys) = self.exposures.remove(exposure) else {
            return 0;
        };
        let mut removed = 0;
        for key in keys {
            if self.tools.remove(&key).is_some() {
                removed += 1;
            }
        }
        removed
    }

    /// Remove every tool of one plugin (unload path).
    pub fn remove_plugin(&mut self, plugin_id: &str) -> usize {
        let keys: Vec<String> = self
            .tools
            .iter()
            .filter(|(_, tool)| tool.spec.plugin_id == plugin_id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &keys {
            self.tools.remove(key);
        }
        for contributed in self.exposures.values_mut() {
            contributed.retain(|key| !keys.contains(key));
        }
        keys.len()
    }

    /// Catalog in a stable order: sorted by qualified name.
    pub fn catalog(&self) -> Vec<McpToolSpec> {
        self.tools.values().map(|tool| tool.spec.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Names currently on the bus, for diagnostics.
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

type Shared = Arc<Mutex<ToolRegistry>>;

/// Handle returned by an explicit exposure; dropping it unregisters the tools.
#[derive(Debug)]
pub struct ExposureHandle {
    id: String,
    plugin_id: String,
    registry: Weak<Mutex<ToolRegistry>>,
}

impl ExposureHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Give up the guard: the tools stay published for the process lifetime.
    pub fn forget(self) {
        std::mem::forget(self);
    }

    pub(crate) fn shared(id: String, plugin_id: String, registry: Shared) -> Self {
        Self {
            id,
            plugin_id,
            registry: Arc::downgrade(&registry),
        }
    }
}

impl Drop for ExposureHandle {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let removed = registry
            .lock()
            .map(|mut guard| guard.remove_exposure(&self.id));
        drop(removed);
    }
}

/// Quote a frame for error messages.
pub fn frame_label(frame: &CallFrame) -> String {
    frame.key()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl McpHandler for Echo {
        async fn call(&self, _ctx: &CallContext, args: serde_json::Value) -> Result<McpCallResult> {
            Ok(McpCallResult {
                ok: true,
                content: args,
                trace_id: String::new(),
            })
        }
    }

    fn spec(plugin: &str, tool: &str) -> McpToolSpec {
        McpToolSpec {
            plugin_id: plugin.to_string(),
            tool: tool.to_string(),
            description: "echo".into(),
            schema: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn duplicate_qualified_name_is_a_conflict() {
        let mut registry = ToolRegistry::new();
        registry
            .insert("e1", spec("ark.a", "run"), Arc::new(Echo))
            .unwrap();
        let err = registry
            .insert("e2", spec("ark.a", "run"), Arc::new(Echo))
            .unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("mcp_tool_conflict"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn tool_cannot_carry_a_foreign_namespace() {
        let mut registry = ToolRegistry::new();
        let err = registry
            .insert("e1", spec("ark.a", "ark.b.run"), Arc::new(Echo))
            .unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("invalid_tool_name"));
        assert!(registry.is_empty());
    }

    #[test]
    fn catalog_is_sorted_and_namespaced() {
        let mut registry = ToolRegistry::new();
        for (plugin, tool) in [("ark.b", "run"), ("ark.a", "run")] {
            registry
                .insert("e1", spec(plugin, tool), Arc::new(Echo))
                .unwrap();
        }
        let names: Vec<String> = registry
            .catalog()
            .into_iter()
            .map(|s| qualify(&s.plugin_id, &s.tool))
            .collect();
        assert_eq!(names, vec!["ark.a.run", "ark.b.run"]);
    }

    #[test]
    fn removing_an_exposure_only_drops_its_own_tools() {
        let mut registry = ToolRegistry::new();
        registry
            .insert("e1", spec("ark.a", "run"), Arc::new(Echo))
            .unwrap();
        registry
            .insert("e2", spec("ark.b", "run"), Arc::new(Echo))
            .unwrap();
        assert_eq!(registry.remove_exposure("e1"), 1);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("ark.b", "run").is_some());
        assert_eq!(registry.remove_exposure("e1"), 0);
    }

    #[test]
    fn handle_drop_unregisters_the_tools() {
        let registry: Shared = Arc::new(Mutex::new(ToolRegistry::new()));
        let handle = {
            // Scope the guard so the registry is not locked while the handle is
            // built: `std::sync::Mutex` is not reentrant and a second lock on the
            // same thread would deadlock the test.
            let mut guard = registry.lock().unwrap();
            guard
                .insert("e1", spec("ark.a", "run"), Arc::new(Echo))
                .unwrap();
            ExposureHandle::shared("e1".into(), "ark.a".into(), registry.clone())
        };
        assert_eq!(registry.lock().unwrap().len(), 1);
        drop(handle);
        assert!(registry.lock().unwrap().is_empty(), "Drop must unregister");
    }
}
