//! Model client port (design §7.2).
//!
//! `endpointFormat` routing, raw SSE framing and retry policy belong to the
//! adapter. The agent sees only typed messages and streamed chunks, which keeps
//! the loop testable with a mock client.

use ark_common::{Result, Usage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: ModelRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool calls issued by this assistant message. Only the assistant side
    /// carries them; adapters serialise them so replayed history keeps the
    /// calls visible to the provider. Old JSON without the field deserialises
    /// as empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRef>,
}

/// One replayed assistant tool call: id, tool name and parsed arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRef {
    pub call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ModelMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ModelRole::System,
            content: content.into(),
            tool_call_id: None,
            name: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ModelRole::User,
            content: content.into(),
            tool_call_id: None,
            name: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ModelRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            name: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant_with_tool_calls(content: impl Into<String>, calls: Vec<ToolCallRef>) -> Self {
        Self {
            role: ModelRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            name: None,
            tool_calls: calls,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_with_tool_calls_survives_a_serde_roundtrip() {
        let message = ModelMessage::assistant_with_tool_calls(
            "running tools",
            vec![ToolCallRef {
                call_id: "c1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "a.txt"}),
            }],
        );
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("tool_calls"));
        let back: ModelMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, message);
    }

    #[test]
    fn old_json_without_tool_calls_deserialises_as_empty() {
        let legacy = r#"{"role":"assistant","content":"hi"}"#;
        let message: ModelMessage = serde_json::from_str(legacy).unwrap();
        assert_eq!(message, ModelMessage::assistant("hi"));
        // An empty list must be omitted so old consumers see no field at all.
        let json = serde_json::to_value(ModelMessage::assistant("hi")).unwrap();
        assert!(json.get("tool_calls").is_none());
    }
}

/// One model request. The tool catalogue travels with every request so the
/// adapter never reads registry state behind the agent's back.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelRequest {
    pub messages: Vec<ModelMessage>,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// A streamed chunk. Deltas are additive; `tool_call` marks a complete call so
/// the loop can dispatch without parsing raw JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelChunk {
    TextDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    ToolCall {
        call_id: String,
        tool: String,
        arguments: serde_json::Value,
    },
    Usage {
        usage: Usage,
    },
}

/// A non-streamed response, used by mock clients and single-shot calls.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelResponse {
    pub text: String,
    #[serde(default)]
    pub tool_calls: Vec<(String, String, serde_json::Value)>,
    #[serde(default)]
    pub usage: Usage,
}

#[async_trait::async_trait]
pub trait ModelClient: Send + Sync {
    /// Stable provider id, e.g. `deepseek` — used in logs and usage records.
    fn provider_id(&self) -> &str;

    fn model(&self) -> &str;

    /// Stream one completion. Implementations must not buffer the whole reply:
    /// long answers are the case where memory peaks in the old runtime.
    async fn stream(
        &self,
        request: ModelRequest,
        sink: tokio::sync::mpsc::Sender<ModelChunk>,
    ) -> Result<ModelResponse>;
}
