//! ark-mcp: Base capability: single MCP egress plus in-process plugin bus (design 7.5.2).
//!
//! Layer folders follow the QDC mapping in design 5.1:
//!
//! * [`domain`] - pure rules: tool namespace, declared visibility, call-chain
//!   guards (depth / cycle / deadlock), JSON-RPC framing. No I/O, so every
//!   safety property is unit-testable.
//! * [`service`] - [`McpBus`] (the [`ark_ports::McpCapability`] implementation)
//!   and the client transports for *external* MCP servers.
//!
//! ## Two directions, one catalog
//!
//! Plugins never start their own MCP server. They publish tools to
//! [`McpBus::catalog`], and this crate is the single thing `arkd` exposes over
//! stdio / streamable-http. Plugin-to-plugin calls go through
//! [`McpBus::call`], which routes in-process: no socket, no address, no protocol
//! version to agree on.
//!
//! ## Bounded recursion
//!
//! Recursion is refused *before* a handler runs, with three distinct errors:
//!
//! * `mcp_depth_exceeded` - more than [`domain::MAX_DEPTH`] nested calls (3);
//! * `mcp_call_cycle` - the same `<plugin>.<tool>` frame is already on the stack
//!   (`A -> B -> A`);
//! * `mcp_deadlock` - two concurrent tasks wait on each other (`A` blocked on
//!   `B` while `B` is blocked on `A`), detected on a shared wait-for graph.
//!
//! ## Namespace and visibility
//!
//! A tool is always published as `<plugin_id>.<tool>`; a conflict fails at
//! registration. A cross-plugin call must appear in the caller's
//! `mcp.consume` list, otherwise it is denied with `mcp_not_permitted`.

#![deny(clippy::all)]

pub mod domain;
pub mod service;

pub use domain::{
    ensure_local, qualify, CallFrame, CallGuard, Chain, ChainError, DeadlockGuard, JsonRpcError,
    JsonRpcId, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, ListToolsResult,
    McpVisibility, ToolDescriptor, VisibilityScope, MAX_CHAIN, MAX_DEPTH,
};
pub use service::{
    CallContext, ExposureHandle, HttpTransportConfig, McpBus, McpHandler, McpServerRegistry,
    McpToolAdapter, McpTransport, RegisteredTool, SseTransport, StdioTransport,
    StreamableHttpTransport, ToolRegistry, TransportKind,
};
