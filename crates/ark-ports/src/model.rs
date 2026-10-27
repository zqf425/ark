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
}

impl ModelMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ModelRole::System,
            content: content.into(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ModelRole::User,
            content: content.into(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ModelRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            name: None,
        }
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
