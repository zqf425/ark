//! The single MCP egress: one catalog, in-process routing (design 7.5.2).
//!
//! # What this bus guarantees
//!
//! * **One catalog.** Every plugin's tools merge into [`McpBus::catalog`], which
//!   also feeds `GET /v1/runtime/capabilities` and the external `tools/list`.
//!   There is no second list.
//! * **In-process routing.** [`McpBus::call`] dispatches straight to the target
//!   plugin's handler. No socket, no JSON-RPC encode/decode on the hot path.
//! * **Bounded recursion.** Depth (default 3), cycle (`A -> B -> A`) and
//!   deadlock (`A` waits for `B` while `B` waits for `A`) are all refused with
//!   explicit errors *before* a handler runs.
//! * **Declared visibility.** A cross-plugin call must appear in the caller's
//!   `mcp.consume`; otherwise it is denied.
//!
//! # Why the chain lives in a task-local
//!
//! [`ark_ports::McpCapability::call`] takes no chain argument, yet nested calls
//! must see their ancestors. The chain is therefore scoped to the *task* that
//! started the ingress call: the handler runs inside
//! [`CHAIN`]`.scope(updated_chain, …)`, so a call it makes inherits the frames
//! already on the stack. Depth and cycle are per task by construction; the
//! wait-for graph ([`DeadlockGuard`]) is shared, which is what concurrent
//! mutual waiting needs.

use crate::domain::call_chain::{Chain, ChainError, DeadlockGuard, MAX_DEPTH};
use crate::domain::visibility::McpVisibility;
use crate::service::registry::{CallContext, ExposureHandle, McpHandler, ToolRegistry};
use ark_common::{ArkError, ArkErrorCode, Result};
use ark_ports::{McpCallResult, McpCapability, McpCatalog, McpToolSpec};
use async_trait::async_trait;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

tokio::task_local! {
    /// Frames active on the current task. Absent outside an ingress call.
    static CHAIN: RefCell<Chain>;
}

/// Handler used for tools exposed through the port-level API, which carries
/// specs but no implementation. Calling such a tool is an explicit error, never
/// a silent success.
struct Unavailable;

#[async_trait]
impl McpHandler for Unavailable {
    async fn call(&self, ctx: &CallContext, _args: serde_json::Value) -> Result<McpCallResult> {
        Err(ArkError::new(
            ArkErrorCode::NotImplemented,
            format!("tool {} has no in-process handler registered", ctx.tool),
        )
        .with_detail("mcp_tool_unavailable"))
    }
}

/// The bus. One instance per runtime, shared by every plugin.
pub struct McpBus {
    registry: Arc<Mutex<ToolRegistry>>,
    visibility: Mutex<McpVisibility>,
    waits: Mutex<DeadlockGuard>,
    max_depth: usize,
}

impl std::fmt::Debug for McpBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpBus")
            .field("max_depth", &self.max_depth)
            .finish_non_exhaustive()
    }
}

impl Default for McpBus {
    /// Depth limit is [`MAX_DEPTH`] (3), matching design 7.5.2.
    fn default() -> Self {
        Self::new(MAX_DEPTH)
    }
}

impl McpBus {
    /// `max_depth` is clamped to `[1, MAX_CHAIN]`; a caller cannot widen it.
    pub fn new(max_depth: usize) -> Self {
        Self {
            registry: Arc::new(Mutex::new(ToolRegistry::new())),
            visibility: Mutex::new(McpVisibility::new()),
            waits: Mutex::new(DeadlockGuard::new()),
            max_depth,
        }
    }

    pub fn max_depth(&self) -> usize {
        self.max_depth
    }

    /// Declare a plugin's `mcp.expose` / `mcp.consume` lists.
    pub fn declare_visibility(
        &self,
        plugin_id: &str,
        expose: &[String],
        consume: &[String],
    ) -> Result<()> {
        self.visibility
            .lock()
            .map_err(|_| ArkError::internal("mcp visibility poisoned"))?
            .declare(plugin_id, expose, consume)
    }

    /// Publish `(spec, handler)` pairs under one exposure group.
    pub fn expose_with_handlers(
        &self,
        plugin_id: &str,
        tools: Vec<(McpToolSpec, Arc<dyn McpHandler>)>,
    ) -> Result<ExposureHandle> {
        let exposure = format!("{}#{}", plugin_id, uuid_like());
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| ArkError::internal("mcp registry poisoned"))?;
        for (spec, handler) in tools {
            registry.insert(&exposure, spec, handler)?;
        }
        drop(registry);
        Ok(ExposureHandle::shared(
            exposure,
            plugin_id.to_string(),
            self.registry.clone(),
        ))
    }

    /// Publish specs without handlers (the port-level shape).
    pub fn expose_specs(&self, plugin_id: &str, tools: Vec<McpToolSpec>) -> Result<ExposureHandle> {
        let pairs = tools
            .into_iter()
            .map(|spec| (spec, Arc::new(Unavailable) as Arc<dyn McpHandler>))
            .collect();
        self.expose_with_handlers(plugin_id, pairs)
    }

    /// Drop every tool contributed by one plugin (unload path).
    pub fn unregister_plugin(&self, plugin_id: &str) -> usize {
        self.registry
            .lock()
            .map(|mut registry| registry.remove_plugin(plugin_id))
            .unwrap_or(0)
            + self
                .visibility
                .lock()
                .map(|mut visibility| {
                    visibility.forget(plugin_id);
                    0
                })
                .unwrap_or(0)
    }

    /// Catalog filtered by what `caller` declared it may consume.
    pub fn catalog_for(&self, caller: &str) -> McpCatalog {
        let allowed = self
            .visibility
            .lock()
            .map(|visibility| visibility.allowed_providers(caller))
            .unwrap_or_default();
        let tools = self
            .registry
            .lock()
            .map(|registry| {
                registry
                    .catalog()
                    .into_iter()
                    .filter(|spec| spec.plugin_id == caller || allowed.contains(&spec.plugin_id))
                    .collect()
            })
            .unwrap_or_default();
        McpCatalog { tools }
    }

    fn authorize(&self, caller: &str, provider: &str, tool: &str) -> Result<()> {
        self.visibility
            .lock()
            .map_err(|_| ArkError::internal("mcp visibility poisoned"))?
            .authorize(caller, provider, tool)
    }

    fn lookup(
        &self,
        provider: &str,
        tool: &str,
    ) -> Result<crate::service::registry::RegisteredTool> {
        self.registry
            .lock()
            .map_err(|_| ArkError::internal("mcp registry poisoned"))?
            .get(provider, tool)
            .cloned()
            .ok_or_else(|| {
                ArkError::not_found(format!("mcp tool {provider}.{tool} is not registered"))
                    .with_detail("mcp_tool_not_found")
            })
    }

    /// Current stack of the calling task, empty outside an ingress call.
    fn current_chain(&self) -> Chain {
        CHAIN
            .try_with(|chain| chain.borrow().clone())
            .unwrap_or_else(|_| Chain::root(self.max_depth))
    }

    fn refuse(&self, err: ChainError) -> ArkError {
        err.into_error()
    }

    /// Ingress + nested routing. Public so tests can drive it directly.
    pub async fn dispatch(
        &self,
        caller: &str,
        provider: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<McpCallResult> {
        self.authorize(caller, provider, tool)?;
        let registered = self.lookup(provider, tool)?;

        let mut chain = self.current_chain();
        chain
            .enter(provider, tool)
            .map_err(|err| self.refuse(err))?;
        let depth = chain.depth();
        let trace_id = chain
            .frames()
            .first()
            .map(|frame| format!("mcp-{}", frame.plugin_id))
            .unwrap_or_else(|| "mcp-root".to_string());
        let frames = chain.trace();

        // Register the wait edge before awaiting the handler so two tasks that
        // wait on each other are refused instead of both blocking forever.
        if caller != provider {
            let mut waits = self
                .waits
                .lock()
                .map_err(|_| ArkError::internal("mcp wait graph poisoned"))?;
            waits
                .register_wait(caller, provider)
                .map_err(|err| self.refuse(err))?;
        }

        let ctx = CallContext {
            caller: caller.to_string(),
            provider: provider.to_string(),
            tool: tool.to_string(),
            trace_id,
            depth,
            chain: frames,
        };

        let result = CHAIN
            .scope(RefCell::new(chain), async {
                registered.handler.call(&ctx, args).await
            })
            .await;

        if caller != provider {
            if let Ok(mut waits) = self.waits.lock() {
                waits.release(caller, provider);
            }
        }

        result.map(|mut value| {
            value.trace_id = ctx.trace_id.clone();
            value
        })
    }
}

#[async_trait]
impl McpCapability for McpBus {
    /// Port-level exposure: registers specs, keeps them for the bus lifetime.
    fn expose(&self, tools: Vec<McpToolSpec>) -> Result<()> {
        let plugin_id = tools
            .first()
            .map(|spec| spec.plugin_id.clone())
            .ok_or_else(|| ArkError::validation("expose requires at least one tool"))?;
        let handle = self.expose_specs(&plugin_id, tools)?;
        handle.forget();
        Ok(())
    }

    async fn call(
        &self,
        caller: &str,
        provider: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<McpCallResult> {
        self.dispatch(caller, provider, tool, args).await
    }

    fn catalog(&self) -> McpCatalog {
        let tools = self
            .registry
            .lock()
            .map(|registry| registry.catalog())
            .unwrap_or_default();
        McpCatalog { tools }
    }
}

/// Monotonic exposure suffix; keeps group ids unique without a uuid crate.
fn uuid_like() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests;
