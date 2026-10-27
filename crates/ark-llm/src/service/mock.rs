//! Scripted model client (design §7.2).
//!
//! The whole point is to exercise the agent loop *without* a provider: the script
//! says which chunks a call produces, and this client forwards them **one at a
//! time** through the sink. A mock that emitted one big chunk would hide every
//! incremental-accumulation bug the real stream can produce.

use std::sync::Mutex;

use ark_common::{ArkError, ArkErrorCode, Result, Usage};
use ark_ports::{ModelChunk, ModelClient, ModelRequest, ModelResponse};
use tokio::sync::mpsc;

/// One scripted call: the chunks to emit, then the settled response.
#[derive(Debug, Clone, Default)]
pub struct ScriptedCall {
    pub chunks: Vec<ModelChunk>,
    /// Error returned after the chunks (a mid-stream failure).
    pub error: Option<String>,
}

impl ScriptedCall {
    /// A pure text answer, split into the given deltas.
    pub fn text<I, S>(deltas: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let chunks = deltas
            .into_iter()
            .map(|delta| ModelChunk::TextDelta {
                delta: delta.into(),
            })
            .collect();
        Self {
            chunks,
            error: None,
        }
    }

    /// One tool call plus the usage sample, as a real provider sends them.
    pub fn tool_call(call_id: &str, tool: &str, arguments: serde_json::Value) -> Self {
        Self {
            chunks: vec![
                ModelChunk::TextDelta {
                    delta: "calling".to_string(),
                },
                ModelChunk::ToolCall {
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    arguments,
                },
                ModelChunk::Usage {
                    usage: Usage::new(10, 5),
                },
            ],
            error: None,
        }
    }

    /// Fail the call after emitting `chunks`.
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            chunks: Vec::new(),
            error: Some(message.into()),
        }
    }

    /// The settled response implied by the chunks.
    pub fn response(&self) -> ModelResponse {
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut usage = Usage::default();
        for chunk in &self.chunks {
            match chunk {
                ModelChunk::TextDelta { delta } => text.push_str(delta),
                ModelChunk::ReasoningDelta { .. } => {}
                ModelChunk::ToolCall {
                    call_id,
                    tool,
                    arguments,
                } => {
                    tool_calls.push((call_id.clone(), tool.clone(), arguments.clone()));
                }
                ModelChunk::Usage { usage: sample } => usage = sample.clone(),
            }
        }
        ModelResponse {
            text,
            tool_calls,
            usage,
        }
    }
}

/// An ordered list of scripted calls.
#[derive(Debug, Clone, Default)]
pub struct Script {
    calls: Vec<ScriptedCall>,
}

impl Script {
    pub fn new(calls: impl IntoIterator<Item = ScriptedCall>) -> Self {
        Self {
            calls: calls.into_iter().collect(),
        }
    }

    /// One text answer split into per-character deltas — the default shape for
    /// streaming tests.
    pub fn text(text: &str) -> Self {
        let deltas: Vec<String> = text.chars().map(String::from).collect();
        Self::new(vec![ScriptedCall::text(deltas)])
    }

    pub fn len(&self) -> usize {
        self.calls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

/// Scripted [`ModelClient`].
#[derive(Debug)]
pub struct MockModelClient {
    provider_id: String,
    model: String,
    script: Mutex<std::collections::VecDeque<ScriptedCall>>,
    calls: Mutex<u32>,
}

impl MockModelClient {
    pub fn new(provider_id: impl Into<String>, model: impl Into<String>, script: Script) -> Self {
        Self {
            provider_id: provider_id.into(),
            model: model.into(),
            script: Mutex::new(script.calls.into_iter().collect()),
            calls: Mutex::new(0),
        }
    }

    /// How many calls the client has served. Used to assert round counts.
    pub fn call_count(&self) -> u32 {
        *self.calls.lock().expect("call counter")
    }

    /// Scripted calls that were never consumed.
    pub fn remaining(&self) -> usize {
        self.script.lock().expect("script queue").len()
    }
}

#[async_trait::async_trait]
impl ModelClient for MockModelClient {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn stream(
        &self,
        _request: ModelRequest,
        sink: mpsc::Sender<ModelChunk>,
    ) -> Result<ModelResponse> {
        *self.calls.lock().expect("call counter") += 1;
        let call = self
            .script
            .lock()
            .expect("script queue")
            .pop_front()
            .ok_or_else(|| ArkError::new(ArkErrorCode::InternalError, "mock script exhausted"))?;

        for chunk in call.chunks.iter().cloned() {
            // A closed sink means the agent interrupted: stop, as a real client
            // would when its reader goes away.
            if sink.send(chunk).await.is_err() {
                return Err(ArkError::new(
                    ArkErrorCode::RuntimeUnavailable,
                    "sink closed",
                ));
            }
        }
        match call.error {
            Some(message) => Err(ArkError::model(message)),
            None => Ok(call.response()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    async fn drain(client: &MockModelClient) -> (Vec<ModelChunk>, Result<ModelResponse>) {
        let (sender, mut receiver) = mpsc::channel(32);
        let call = client.stream(ModelRequest::default(), sender).await;
        let mut chunks = Vec::new();
        while let Ok(chunk) = receiver.try_recv() {
            chunks.push(chunk);
        }
        (chunks, call)
    }

    #[tokio::test]
    async fn deltas_are_emitted_one_by_one() {
        let client = MockModelClient::new("mock", "m", Script::text("hey"));
        let (chunks, response) = drain(&client).await;
        assert_eq!(chunks.len(), 3, "one chunk per character");
        assert_eq!(response.unwrap().text, "hey");
    }

    #[tokio::test]
    async fn tool_calls_and_usage_are_preserved() {
        let script = Script::new(vec![ScriptedCall::tool_call(
            "c1",
            "read",
            serde_json::json!({"path": "a"}),
        )]);
        let client = MockModelClient::new("mock", "m", script);
        let (chunks, response) = drain(&client).await;
        let response = response.unwrap();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].1, "read");
        assert_eq!(response.usage.total_tokens, 15);
        assert!(chunks
            .iter()
            .any(|c| matches!(c, ModelChunk::ToolCall { .. })));
    }

    #[tokio::test]
    async fn scripted_errors_surface_after_the_chunks() {
        let script = Script::new(vec![ScriptedCall {
            chunks: vec![ModelChunk::TextDelta {
                delta: "par".into(),
            }],
            error: Some("stream broke".into()),
        }]);
        let client = MockModelClient::new("mock", "m", script);
        let (chunks, response) = drain(&client).await;
        assert_eq!(chunks.len(), 1);
        assert!(response.unwrap_err().message.contains("stream broke"));
    }

    #[tokio::test]
    async fn calls_advance_through_the_script() {
        let script = Script::new(vec![ScriptedCall::text(["a"]), ScriptedCall::text(["b"])]);
        let client = MockModelClient::new("mock", "m", script);
        assert_eq!(client.remaining(), 2);
        let (_, first) = drain(&client).await;
        assert_eq!(first.unwrap().text, "a");
        let (_, second) = drain(&client).await;
        assert_eq!(second.unwrap().text, "b");
        assert_eq!(client.call_count(), 2);
        assert_eq!(client.remaining(), 0);
    }

    #[tokio::test]
    async fn exhausted_script_is_an_error_not_a_hang() {
        let client = MockModelClient::new("mock", "m", Script::default());
        let (_, response) = drain(&client).await;
        assert!(response.unwrap_err().message.contains("exhausted"));
    }

    #[tokio::test]
    async fn provider_and_model_are_reported() {
        let client = MockModelClient::new("deepseek", "deepseek-chat", Script::default());
        assert_eq!(client.provider_id(), "deepseek");
        assert_eq!(client.model(), "deepseek-chat");
    }

    #[tokio::test]
    async fn closed_sink_stops_the_stream() {
        let client = MockModelClient::new("mock", "m", Script::text("abcdef"));
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let response = client.stream(ModelRequest::default(), sender).await;
        assert!(response.is_err());
    }
}
