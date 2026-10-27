//! Pure MCP rules: naming, visibility, call-chain guards, JSON-RPC framing.
//!
//! Everything here is I/O-free so the safety properties (namespace conflicts,
//! depth limit, cycle and deadlock detection) are unit-testable without a
//! socket or a child process.

pub mod call_chain;
pub mod jsonrpc;
pub mod namespace;
pub mod server_entry;
pub mod visibility;

pub use call_chain::{
    reset_for_tests, CallFrame, CallGuard, Chain, ChainError, DeadlockGuard, MAX_CHAIN, MAX_DEPTH,
};
pub use jsonrpc::{
    decode_call_params, CallToolParams, CallToolResult, ContentBlock, JsonRpcError, JsonRpcId,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, ListToolsResult, ToolDescriptor,
    METHOD_INITIALIZE, METHOD_TOOLS_CALL, METHOD_TOOLS_LIST,
};
pub use namespace::{ensure_local, qualify, validate_plugin_id, validate_tool_name};
pub use server_entry::{
    endpoint_url, normalize as normalize_server_entry, validate as validate_server_entry,
};
pub use server_entry::{validate_id as validate_server_id, MAX_ID_LEN};
pub use visibility::{declare, McpVisibility, VisibilityScope};
