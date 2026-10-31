//! One MCP client session against an external server.
//!
//! The session owns a [`McpTransport`] and serialises requests on it. The
//! serialisation is not cosmetic: both shipped transports read one response at a
//! time and *discard* responses whose id does not match, so two in-flight
//! requests on the same stdio child would lose each other's answers. One gate per
//! server keeps ids and frames in lockstep without touching the transports.

use crate::domain::jsonrpc::{
    CallToolResult, JsonRpcId, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    ListToolsResult, ToolDescriptor, METHOD_INITIALIZE, METHOD_TOOLS_CALL, METHOD_TOOLS_LIST,
};
use crate::service::transport::{McpTransport, StdioTransport, StreamableHttpTransport};
use ark_common::{ArkError, ArkErrorCode, Result};
use ark_contract::{McpServerEntry, McpTransportKind};
use ark_http::HttpBus;
use serde_json::json;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::egress::ServerEgress;

/// Protocol revision this client asks for; servers may negotiate down.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// Client identity reported during `initialize`.
const CLIENT_NAME: &str = "ark";
/// Guard against a server that never stops paginating `tools/list`.
const MAX_TOOL_PAGES: usize = 50;

/// Lazy handle to the shared HTTP egress. `Err` carries a readable reason.
pub(crate) type BusSlot = std::result::Result<Arc<HttpBus>, String>;

/// A connected server.
pub(crate) struct ServerSession {
    pub(crate) server: String,
    transport: Arc<dyn McpTransport>,
    gate: Mutex<()>,
    next_id: AtomicI64,
}

/// Open session plus the tool catalogue read during connect.
pub(crate) struct Connected {
    pub(crate) session: Arc<ServerSession>,
    pub(crate) tools: Vec<ToolDescriptor>,
}

impl std::fmt::Debug for ServerSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerSession")
            .field("server", &self.server)
            .field("transport", &self.transport.kind().as_str())
            .finish_non_exhaustive()
    }
}

/// Connect, handshake and list tools. The caller applies the timeout.
pub(crate) async fn connect(entry: &McpServerEntry, bus: &BusSlot) -> Result<Connected> {
    let transport = build_transport(entry, bus).await?;
    let session = Arc::new(ServerSession {
        server: entry.id.clone(),
        transport,
        gate: Mutex::new(()),
        next_id: AtomicI64::new(1),
    });
    session.handshake(entry).await?;
    let tools = session.list_tools().await?;
    Ok(Connected { session, tools })
}

async fn build_transport(entry: &McpServerEntry, bus: &BusSlot) -> Result<Arc<dyn McpTransport>> {
    match entry.transport {
        McpTransportKind::Stdio => spawn_stdio(entry).await,
        // SSE is contract-complete but not implemented: reject before any
        // network I/O instead of letting connect fail later with a generic
        // transport error, so callers can switch to streamable_http.
        McpTransportKind::Sse => Err(ArkError::validation(format!(
            "mcp server {} uses the sse transport, which is not implemented yet; use streamable_http",
            entry.id
        ))
        .with_detail("mcp_sse_unsupported")),
        McpTransportKind::StreamableHttp => {
            let bus = bus.as_ref().map_err(|message| {
                ArkError::new(ArkErrorCode::RuntimeUnavailable, message.clone())
                    .with_detail("mcp_egress_unavailable")
            })?;
            let egress = ServerEgress::new(bus.clone(), entry)?;
            let config = ServerEgress::config(entry, egress.plugin_id())?;
            Ok(Arc::new(StreamableHttpTransport::new(egress, config)?))
        }
    }
}

/// Spawn a stdio server through the shared transport.
///
/// Only `command` and `args` are forwarded: [`StdioTransport::spawn`] takes no
/// environment map, and writing into the current process environment to work
/// around that would leak one server's secrets into every later child. `env` is
/// still validated and persisted so the value survives a transport upgrade.
async fn spawn_stdio(entry: &McpServerEntry) -> Result<Arc<dyn McpTransport>> {
    let command = entry.command.as_deref().map(str::trim).unwrap_or_default();
    if command.is_empty() {
        return Err(ArkError::validation(format!(
            "mcp server {} uses the stdio transport and requires a command",
            entry.id
        ))
        .with_detail("invalid_command"));
    }
    let transport = StdioTransport::spawn(command, &entry.args).await?;
    Ok(Arc::new(transport))
}

impl ServerSession {
    async fn handshake(&self, entry: &McpServerEntry) -> Result<()> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") },
        });
        let response = self.request(METHOD_INITIALIZE, Some(params)).await?;
        response.into_result().map_err(|err| {
            ArkError::new(
                err.code,
                format!(
                    "mcp server {} refused initialize: {}",
                    entry.id, err.message
                ),
            )
            .with_detail("mcp_handshake_failed")
        })?;
        // Best effort: servers that do not implement the notification ignore it,
        // and a write failure surfaces on the first real request anyway.
        let _ = self
            .transport
            .notify(JsonRpcNotification::new("notifications/initialized", None))
            .await;
        Ok(())
    }

    /// Send one request; ids are unique per session and the gate keeps the
    /// request/response pairs aligned on single-stream transports.
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let _gate = self.gate.lock().await;
        let id = JsonRpcId::Number(self.next_id.fetch_add(1, Ordering::SeqCst));
        self.transport
            .request(JsonRpcRequest::new(id, method, params))
            .await
    }

    /// Read every page of `tools/list`.
    async fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TOOL_PAGES {
            let params = cursor.as_ref().map(|value| json!({ "cursor": value }));
            let response = self.request(METHOD_TOOLS_LIST, params).await?;
            let page: ListToolsResult = decode(response, "tools/list")?;
            tools.extend(page.tools);
            match page.next_cursor {
                Some(next) if !next.is_empty() => cursor = Some(next),
                _ => return Ok(tools),
            }
        }
        Err(ArkError::new(
            ArkErrorCode::RuntimeUnavailable,
            format!(
                "mcp server {} returned more than {MAX_TOOL_PAGES} tool pages",
                self.server
            ),
        )
        .with_detail("mcp_tool_page_limit"))
    }

    /// Forward one `tools/call`.
    ///
    /// A remote JSON-RPC error becomes a `ToolExecutionFailed` error carrying the
    /// server's own message; the adapter above turns it into `ToolOutput::err` so
    /// the model can read what went wrong and correct itself.
    pub(crate) async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<CallToolResult> {
        let params = json!({ "name": name, "arguments": arguments });
        let response = self.request(METHOD_TOOLS_CALL, Some(params)).await?;
        let value = response.into_result().map_err(|err| {
            ArkError::new(ArkErrorCode::ToolExecutionFailed, err.message)
                .with_detail("mcp_remote_error")
        })?;
        Ok(decode_tool_result(value))
    }

    /// Release the transport. Safe to call twice.
    #[allow(dead_code)]
    pub(crate) async fn close(&self) -> Result<()> {
        self.transport.close().await
    }
}

fn decode<T: serde::de::DeserializeOwned>(response: JsonRpcResponse, what: &str) -> Result<T> {
    let value = response.into_result()?;
    serde_json::from_value(value).map_err(|err| {
        ArkError::validation(format!("invalid {what} payload from mcp server: {err}"))
            .with_detail("mcp_transport_frame")
    })
}

/// A server that answers with a non-standard body still gets its text back to
/// the model instead of an opaque decode error.
///
/// `CallToolResult` deserializes successfully from almost any JSON object because
/// both of its fields are defaulted, so "decoded" alone is not proof of a usable
/// answer: an unknown shape yields empty content. An empty body therefore falls
/// back to the raw payload, which beats handing the model a blank tool result.
fn decode_tool_result(value: serde_json::Value) -> CallToolResult {
    match serde_json::from_value::<CallToolResult>(value.clone()) {
        Ok(result) if !result.content.is_empty() => result,
        _ => CallToolResult::text(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;

    #[test]
    fn non_standard_tool_result_keeps_its_text() {
        let result = decode_tool_result(json!({ "unexpected": true }));
        assert!(!result.is_error);
        assert!(result.joined_text().contains("unexpected"));
    }

    #[test]
    fn standard_tool_result_is_decoded() {
        let result = decode_tool_result(json!({
            "content": [{ "type": "text", "text": "hello" }],
            "isError": false
        }));
        assert_eq!(result.joined_text(), "hello");
    }

    #[test]
    fn remote_error_detail_survives() {
        let err = ArkError::new(ArkErrorCode::ToolExecutionFailed, "mcp error -32603: boom")
            .with_detail("mcp_remote_error");
        assert_eq!(err.detail.as_deref(), Some("mcp_remote_error"));
        assert!(err.message.contains("boom"));
    }

    #[test]
    fn sse_transport_is_rejected_before_connect() {
        let entry = McpServerEntry {
            id: "legacy".into(),
            transport: McpTransportKind::Sse,
            enabled: true,
            command: None,
            args: vec![],
            url: Some("https://mcp.example.com/sse".into()),
            allow_tools: vec![],
            env: Default::default(),
            headers: Default::default(),
        };
        let Err(err) = tokio_test_block_on(build_transport(&entry, &Err("no egress".into())))
        else {
            panic!("sse transport must be rejected before connect");
        };
        assert_eq!(err.code, ArkErrorCode::ValidationError);
        assert_eq!(err.detail.as_deref(), Some("mcp_sse_unsupported"));
        assert!(err.message.contains("streamable_http"));
    }

    /// The Sse arm of `build_transport` returns before its first await point,
    /// so a minimal current-thread runtime is enough to drive it in a unit
    /// test without pulling extra dev-dependencies.
    fn tokio_test_block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(future)
    }

    #[test]
    fn error_response_becomes_tool_execution_failure() {
        let response = JsonRpcResponse::err(
            JsonRpcId::Number(1),
            crate::domain::jsonrpc::JsonRpcError::new(-32602, "bad arguments"),
        );
        let err = response
            .into_result()
            .map_err(|err| {
                ArkError::new(ArkErrorCode::ToolExecutionFailed, err.message)
                    .with_detail("mcp_remote_error")
            })
            .unwrap_err();
        assert_eq!(err.code, ArkErrorCode::ToolExecutionFailed);
        assert_eq!(err.detail.as_deref(), Some("mcp_remote_error"));
        assert!(err.message.contains("bad arguments"));
    }
}
