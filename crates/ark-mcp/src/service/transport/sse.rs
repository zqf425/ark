//! SSE transport skeleton (design 7.6.1).
//!
//! The legacy MCP "SSE" transport is two channels: a long-lived `GET` stream
//! carrying server messages and a separate `POST` endpoint for client messages.
//! This type is deliberately **type-complete but not wired**: stdio and
//! streamable-http are the implemented pair (7.6.3), and SSE client protocol
//! support waits on a dedicated requirement design; adding a half-working
//! third channel would be worse than an explicit error.
//!
//! It exists so:
//!
//! * the [`McpTransport`] surface is proven to cover all three kinds;
//! * the wire contract keeps the `sse` variant instead of a silent enum
//!   divergence with existing persisted configs.
//!
//! Nothing constructs this transport today: the session layer rejects
//! `McpTransportKind::Sse` with a validation error (detail
//! `mcp_sse_unsupported`) before any connection attempt, pointing callers at
//! `streamable_http`.

use crate::domain::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::service::transport::http_shared::HttpTransportConfig;
use crate::service::transport::{unsupported, McpTransport, TransportKind};
use ark_common::Result;
use ark_ports::HttpCapability;
use async_trait::async_trait;
use std::sync::Arc;

/// Two-channel SSE transport (skeleton).
pub struct SseTransport {
    http: Arc<dyn HttpCapability>,
    config: HttpTransportConfig,
}

impl std::fmt::Debug for SseTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseTransport")
            .field("plugin_id", &self.config.plugin_id)
            .field("endpoint", &self.config.endpoint)
            .finish_non_exhaustive()
    }
}

impl SseTransport {
    pub fn new(http: Arc<dyn HttpCapability>, config: HttpTransportConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { http, config })
    }

    pub fn config(&self) -> &HttpTransportConfig {
        &self.config
    }

    /// The shared egress is held so wiring this transport later does not need a
    /// constructor change; the field is otherwise unused in v1.
    pub fn http(&self) -> &Arc<dyn HttpCapability> {
        &self.http
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Sse
    }

    async fn request(&self, _req: JsonRpcRequest) -> Result<JsonRpcResponse> {
        Err(unsupported("mcp sse transport"))
    }

    async fn notify(&self, _note: JsonRpcNotification) -> Result<()> {
        Err(unsupported("mcp sse transport"))
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}
