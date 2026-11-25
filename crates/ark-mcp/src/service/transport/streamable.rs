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
use ark_ports::{HeaderMap, HttpCapability, OutboundRequest, OutboundResponse};
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
            follow_redirects: None,
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

    #[cfg(test)]
    fn parse_payload(raw: &str) -> Result<serde_json::Value> {
        // Kept for direct parser tests. The live transport always selects the
        // decoder from Content-Type and never inspects JSON string contents.
        if raw.trim_start().starts_with("event:")
            || raw.lines().any(|line| line.starts_with("data:"))
        {
            parse_sse_payloads(raw)?
                .into_iter()
                .next()
                .ok_or_else(no_sse_data)
        } else {
            parse_json_payload(raw)
        }
    }
}

fn no_sse_data() -> ArkError {
    ArkError::new(
        ArkErrorCode::RuntimeUnavailable,
        "mcp SSE frame carried no data",
    )
    .with_detail("mcp_transport_frame")
}

fn invalid_payload(message: String) -> ArkError {
    ArkError::validation(message).with_detail("mcp_transport_frame")
}

fn parse_json_payload(raw: &str) -> Result<serde_json::Value> {
    serde_json::from_str(raw)
        .map_err(|error| invalid_payload(format!("invalid mcp JSON payload: {error}")))
}

fn parse_sse_payloads(raw: &str) -> Result<Vec<serde_json::Value>> {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut payloads = Vec::new();
    for frame in normalized.split("\n\n") {
        let data: Vec<&str> = frame
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|line| line.strip_prefix(' ').unwrap_or(line))
            .collect();
        if data.is_empty() {
            continue;
        }
        let joined = data.join("\n");
        payloads.push(
            serde_json::from_str(&joined)
                .map_err(|error| invalid_payload(format!("invalid mcp SSE payload: {error}")))?,
        );
    }
    if payloads.is_empty() {
        Err(no_sse_data())
    } else {
        Ok(payloads)
    }
}

fn response_content_type(response: &OutboundResponse) -> String {
    response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value)
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
        .unwrap_or_default()
}

fn response_id_error() -> ArkError {
    ArkError::new(
        ArkErrorCode::ValidationError,
        "mcp response id did not match the request",
    )
    .with_detail("mcp_transport_response_id")
}

fn parse_response(
    raw: &str,
    content_type: &str,
    expected_id: &JsonRpcId,
) -> Result<JsonRpcResponse> {
    let payloads = if content_type.eq_ignore_ascii_case("text/event-stream") {
        parse_sse_payloads(raw)?
    } else {
        vec![parse_json_payload(raw)?]
    };
    for payload in payloads {
        let Some(value) = payload.get("id") else {
            continue;
        };
        let id =
            serde_json::from_value::<JsonRpcId>(value.clone()).map_err(|_| response_id_error())?;
        if id != *expected_id {
            continue;
        }
        return serde_json::from_value(payload)
            .map_err(|error| invalid_payload(format!("invalid mcp response: {error}")));
    }
    Err(response_id_error())
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
        let expected_id = req.id.clone();
        let body = serde_json::to_string(&req)?;
        let outbound = self.post(body).await?;
        let content_type = response_content_type(&outbound);
        parse_response(&outbound.body, &content_type, &expected_id)
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
    use ark_ports::{OutboundRequest, OutboundResponse};

    struct StubHttp {
        response: OutboundResponse,
    }

    #[async_trait]
    impl HttpCapability for StubHttp {
        async fn request(
            &self,
            _plugin_id: &str,
            _req: OutboundRequest,
        ) -> Result<OutboundResponse> {
            Ok(self.response.clone())
        }
    }

    fn transport(body: &str, content_type: &str) -> StreamableHttpTransport {
        let http = Arc::new(StubHttp {
            response: OutboundResponse {
                status: 200,
                headers: HeaderMap::from([("content-type".to_string(), content_type.to_string())]),
                body: body.to_string(),
            },
        });
        StreamableHttpTransport::new(
            http,
            HttpTransportConfig::new("ark.example", "https://mcp.example.com/rpc"),
        )
        .unwrap()
    }

    #[test]
    fn json_string_containing_data_is_not_mistaken_for_sse() {
        let value = StreamableHttpTransport::parse_payload(
            r#"{"jsonrpc":"2.0","id":1,"result":{"text":"data: literal"}}"#,
        )
        .unwrap();
        assert_eq!(value["result"]["text"], "data: literal");
    }

    #[test]
    fn multiline_sse_data_is_joined_before_json_decoding() {
        let raw = "event: message\ndata: {\"jsonrpc\":\"2.0\",\ndata: \"id\":2,\"result\":{}}\n\n";
        let value = StreamableHttpTransport::parse_payload(raw).unwrap();
        assert_eq!(value["id"], 2);
    }

    #[tokio::test]
    async fn response_id_must_match_the_request_amid_sse_frames() {
        let transport = transport(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n\
             event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{}}\n\n\
             event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n\n",
            "text/event-stream",
        );
        let response = transport
            .request(JsonRpcRequest::tools_list(JsonRpcId::Number(1), None))
            .await
            .unwrap();
        assert_eq!(response.id, JsonRpcId::Number(1));
        assert_eq!(response.result, Some(serde_json::json!({"tools": []})));
    }

    #[tokio::test]
    async fn an_unrelated_response_id_is_rejected() {
        let transport = transport(
            r#"{"jsonrpc":"2.0","id":2,"result":{}}"#,
            "application/json",
        );
        let err = transport
            .request(JsonRpcRequest::tools_list(JsonRpcId::Number(1), None))
            .await
            .unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("mcp_transport_response_id"));
    }

    #[tokio::test]
    async fn content_type_not_body_heuristics_selects_the_decoder() {
        let transport = transport(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n",
            "application/json",
        );
        assert!(transport
            .request(JsonRpcRequest::tools_list(JsonRpcId::Number(1), None))
            .await
            .is_err());
    }

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
