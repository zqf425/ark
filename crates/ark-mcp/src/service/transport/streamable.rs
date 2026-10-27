//! streamable-http transport (design 7.6.1, 7.6.3).
//!
//! One endpoint, one POST per message. The response is either a plain
//! `application/json` body or an `text/event-stream` whose `data:` frame holds
//! the JSON-RPC response. Both shapes are handled here.
//!
//! The request always goes through [`ark_ports::HttpCapability`]: remote MCP
//! servers inherit the bus proxy/TLS/retry/allowlist/credential policy instead
//! of building a second HTTP client.

use crate::domain::jsonrpc::{JsonRpcId, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::service::transport::{HttpTransportConfig, McpTransport, TransportKind};
use ark_common::{ArkError, ArkErrorCode, Result};
use ark_ports::{HeaderMap, HttpCapability, OutboundRequest};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::Mutex;

/// POST-based MCP transport.
pub struct StreamableHttpTransport {
    http: Arc<dyn HttpCapability>,
    config: HttpTransportConfig,
    next_id: Mutex<i64>,
    session: Mutex<Option<String>>,
    closed: Mutex<bool>,
}

impl std::fmt::Debug for StreamableHttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamableHttpTransport")
            .field("plugin_id", &self.config.plugin_id)
            .field("endpoint", &self.config.endpoint)
            .finish_non_exhaustive()
    }
}

impl StreamableHttpTransport {
    pub fn new(http: Arc<dyn HttpCapability>, config: HttpTransportConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            http,
            config,
            next_id: Mutex::new(1),
            session: Mutex::new(None),
            closed: Mutex::new(false),
        })
    }

    pub fn next_id(&self) -> JsonRpcId {
        let mut guard = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        let id = *guard;
        *guard += 1;
        JsonRpcId::Number(id)
    }

    /// `mcp-session-id` captured from a previous response, if any.
    pub fn session_id(&self) -> Option<String> {
        self.session
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or(None)
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers.insert(
            "accept".to_string(),
            "application/json, text/event-stream".to_string(),
        );
        headers.insert("mcp-protocol-version".to_string(), "2025-06-18".to_string());
        if let Some(session) = self.session_id() {
            headers.insert("mcp-session-id".to_string(), session);
        }
        headers
    }

    /// Send one body and return the raw response payload.
    async fn post(&self, body: String) -> Result<ark_ports::OutboundResponse> {
        let request = OutboundRequest {
            method: "POST".to_string(),
            url: self.config.endpoint.clone(),
            headers: self.headers(),
            body: Some(body),
            timeout_ms: self.config.timeout_ms,
            idempotency_key: None,
            credential: self.config.credential.clone(),
        };
        let response = self.http.request(&self.config.plugin_id, request).await?;
        if let Some(session) = response.headers.get("mcp-session-id") {
            if let Ok(mut guard) = self.session.lock() {
                *guard = Some(session.clone());
            }
        }
        if !(200..300).contains(&response.status) {
            return Err(ArkError::new(
                ArkErrorCode::RuntimeUnavailable,
                format!("mcp endpoint returned http {}", response.status),
            )
            .with_detail("mcp_transport_status"));
        }
        Ok(response)
    }

    fn parse_payload(raw: &str) -> Result<serde_json::Value> {
        if raw.trim_start().starts_with("event:") || raw.contains("data:") {
            let data = raw
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim)
                .find(|line| !line.is_empty())
                .ok_or_else(|| {
                    ArkError::new(
                        ArkErrorCode::RuntimeUnavailable,
                        "mcp SSE frame carried no data",
                    )
                    .with_detail("mcp_transport_frame")
                })?;
            return serde_json::from_str(data).map_err(|err| {
                ArkError::validation(format!("invalid mcp SSE payload: {err}"))
                    .with_detail("mcp_transport_frame")
            });
        }
        serde_json::from_str(raw).map_err(|err| {
            ArkError::validation(format!("invalid mcp JSON payload: {err}"))
                .with_detail("mcp_transport_frame")
        })
    }
}

#[async_trait]
impl McpTransport for StreamableHttpTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::StreamableHttp
    }

    async fn request(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse> {
        if *self
            .closed
            .lock()
            .map_err(|_| ArkError::internal("mcp state poisoned"))?
        {
            return Err(
                ArkError::new(ArkErrorCode::RuntimeUnavailable, "mcp transport closed")
                    .with_detail("mcp_transport_closed"),
            );
        }
        let body = serde_json::to_string(&req)?;
        let response = self.post(body).await?;
        let value = Self::parse_payload(&response.body)?;
        let response: JsonRpcResponse = serde_json::from_value(value).map_err(|err| {
            ArkError::validation(format!("invalid mcp response: {err}"))
                .with_detail("mcp_transport_frame")
        })?;
        Ok(response)
    }

    async fn notify(&self, note: JsonRpcNotification) -> Result<()> {
        let body = serde_json::to_string(&note)?;
        self.post(body).await.map(|_| ())
    }

    async fn close(&self) -> Result<()> {
        if let Ok(mut closed) = self.closed.lock() {
            *closed = true;
        }
        if let Ok(mut session) = self.session.lock() {
            *session = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_body_is_parsed_directly() {
        let value = StreamableHttpTransport::parse_payload(r#"{"jsonrpc":"2.0","id":1}"#).unwrap();
        assert_eq!(value["id"], 1);
    }

    #[test]
    fn sse_frame_is_unwrapped() {
        let raw = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2}\n\n";
        let value = StreamableHttpTransport::parse_payload(raw).unwrap();
        assert_eq!(value["id"], 2);
    }

    #[test]
    fn empty_sse_frame_is_an_error() {
        let err = StreamableHttpTransport::parse_payload("event: message\ndata:\n").unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("mcp_transport_frame"));
    }
}
