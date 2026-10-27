//! JSON-RPC 2.0 framing for the MCP egress (design 7.6.1).
//!
//! Only the messages MCP actually uses are modelled: `initialize`-style
//! requests, `tools/list`, `tools/call`, notifications and error responses. The
//! in-process bus does **not** serialize through these types (design 7.5.2), it
//! only uses them at the transport boundary.

use ark_common::{ArkError, Result};
use serde::{Deserialize, Serialize};

pub const JSONRPC_VERSION: &str = "2.0";
pub const METHOD_TOOLS_LIST: &str = "tools/list";
pub const METHOD_TOOLS_CALL: &str = "tools/call";
pub const METHOD_INITIALIZE: &str = "initialize";

/// Request id: number or string, kept opaque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    Text(String),
}

/// Outbound request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(
        id: JsonRpcId,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.into(),
            params,
        }
    }

    pub fn tools_list(id: JsonRpcId, cursor: Option<String>) -> Self {
        let params = cursor.map(|c| serde_json::json!({ "cursor": c }));
        Self::new(id, METHOD_TOOLS_LIST, params)
    }

    pub fn tools_call(id: JsonRpcId, name: &str, arguments: serde_json::Value) -> Self {
        Self::new(
            id,
            METHOD_TOOLS_CALL,
            Some(serde_json::json!({ "name": name, "arguments": arguments })),
        )
    }
}

/// Notification (no id, no response expected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params,
        }
    }
}

/// Error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcError {
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// Response: exactly one of `result` / `error` is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn ok(id: JsonRpcId, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: JsonRpcId, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }

    pub fn into_result(self) -> Result<serde_json::Value> {
        if let Some(error) = self.error {
            return Err(ArkError::new(
                ark_common::ArkErrorCode::ToolExecutionFailed,
                format!("mcp error {}: {}", error.code, error.message),
            ));
        }
        self.result
            .ok_or_else(|| ArkError::internal("mcp response carried neither result nor error"))
    }
}

/// `tools/list` result entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: serde_json::Value,
}

/// `tools/list` result envelope.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListToolsResult {
    #[serde(default)]
    pub tools: Vec<ToolDescriptor>,
    #[serde(
        rename = "nextCursor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
}

/// `tools/call` params, decoded by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// `tools/call` result: content blocks plus `isError`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
}

impl CallToolResult {
    pub fn text(value: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text { text: value.into() }],
            is_error: false,
        }
    }

    /// Flatten text blocks; used when handing a remote result back to a plugin.
    pub fn joined_text(&self) -> String {
        self.content
            .iter()
            .map(|ContentBlock::Text { text }| text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Decode a `tools/call` params object.
pub fn decode_call_params(params: Option<&serde_json::Value>) -> Result<CallToolParams> {
    let params = params.ok_or_else(|| ArkError::validation("tools/call requires params"))?;
    serde_json::from_value(params.clone()).map_err(|err| {
        ArkError::validation(format!("invalid tools/call params: {err}"))
            .with_detail("invalid_params")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_request_roundtrips() {
        let request = JsonRpcRequest::tools_list(JsonRpcId::Number(1), Some("c1".into()));
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"method\":\"tools/list\""));
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        let back: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, request);
    }

    #[test]
    fn tools_call_request_carries_arguments() {
        let request = JsonRpcRequest::tools_call(
            JsonRpcId::Text("abc".into()),
            "jira.search",
            serde_json::json!({ "jql": "project = ARK" }),
        );
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["method"], "tools/call");
        assert_eq!(json["params"]["name"], "jira.search");
        assert_eq!(json["params"]["arguments"]["jql"], "project = ARK");
    }

    #[test]
    fn decode_call_params_rejects_missing_name() {
        let err = decode_call_params(Some(&serde_json::json!({ "arguments": {} }))).unwrap_err();
        assert_eq!(err.detail.as_deref(), Some("invalid_params"));
    }

    #[test]
    fn list_tools_result_uses_wire_field_names() {
        let result = ListToolsResult {
            tools: vec![ToolDescriptor {
                name: "jira.search".into(),
                title: None,
                description: "search issues".into(),
                input_schema: serde_json::json!({ "type": "object" }),
            }],
            next_cursor: None,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["tools"][0]["inputSchema"]["type"], "object");
        assert!(
            json.get("nextCursor").is_none(),
            "absent cursor must not be serialized"
        );
    }

    #[test]
    fn error_response_surfaces_as_error() {
        let response = JsonRpcResponse::err(
            JsonRpcId::Number(2),
            JsonRpcError::new(JsonRpcError::METHOD_NOT_FOUND, "no such tool"),
        );
        let err = response.into_result().unwrap_err();
        assert!(err.message.contains("no such tool"));
    }

    #[test]
    fn call_tool_result_flattens_text() {
        let result = CallToolResult::text("ok");
        assert_eq!(result.joined_text(), "ok");
        assert!(!result.is_error);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["content"][0]["type"], "text");
    }

    #[test]
    fn notification_has_no_id() {
        let json =
            serde_json::to_value(JsonRpcNotification::new("notifications/initialized", None))
                .unwrap();
        assert!(json.get("id").is_none());
    }
}
