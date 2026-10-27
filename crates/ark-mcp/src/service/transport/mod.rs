//! Transport layer for the MCP client direction (design 7.6.1).
//!
//! The in-process bus never uses these types: plugin-to-plugin calls are direct
//! handler invocations. Transports exist for *external* MCP servers, and the
//! HTTP-based ones deliberately go through [`ark_ports::HttpCapability`] so a
//! remote MCP server inherits the bus proxy/TLS/retry/allowlist policy instead
//! of creating a second `reqwest::Client`.

use crate::domain::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use ark_common::{ArkError, ArkErrorCode, Result};
use async_trait::async_trait;

/// Which transport a connection uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Stdio,
    Sse,
    StreamableHttp,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportKind::Stdio => "stdio",
            TransportKind::Sse => "sse",
            TransportKind::StreamableHttp => "streamable-http",
        }
    }
}

/// One MCP connection. Implementations own framing only; catalog semantics live
/// in [`crate::service::McpBus`].
#[async_trait]
pub trait McpTransport: Send + Sync {
    fn kind(&self) -> TransportKind;

    /// Send one request and read the matching response.
    async fn request(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// Fire-and-forget notification (no response is expected).
    async fn notify(&self, note: JsonRpcNotification) -> Result<()>;

    /// Release the connection; safe to call twice.
    async fn close(&self) -> Result<()>;
}

mod http_shared;
mod sse;
mod stdio;
mod streamable;

pub use http_shared::HttpTransportConfig;
pub use sse::SseTransport;
pub use stdio::StdioTransport;
pub use streamable::StreamableHttpTransport;

/// Common denial for transports that are type-complete but not wired yet.
pub(crate) fn unsupported(what: &str) -> ArkError {
    ArkError::new(
        ArkErrorCode::NotImplemented,
        format!("{what} is not wired in v1"),
    )
    .with_detail("mcp_transport_unavailable")
}
